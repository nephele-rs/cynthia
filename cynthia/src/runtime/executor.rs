use std::future::Future;
use std::marker::PhantomData;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Poll, Waker};

use crate::future::{future, prelude::*};
use crate::platform::queue::ConcurrentQueue;
use crate::platform::waker::Sleeper;
use crate::runtime::task::Runnable;
use crate::utils::{rander, Colo};
use crate::io::reactor::Reactor;
use crate::runtime::task::{self, Task};

#[derive(Debug)]
pub struct State {
    pub queue: ConcurrentQueue<Runnable>,
    pub local_queues: RwLock<Vec<Arc<ConcurrentQueue<Runnable>>>>,
    pub notified: AtomicBool,
    pub sleepers: Mutex<Sleeper>,
    pub active: Mutex<Colo<Waker>>,
}

impl State {
    pub fn new() -> State {
        State {
            queue: ConcurrentQueue::unbounded(),
            local_queues: RwLock::new(Vec::new()),
            notified: AtomicBool::new(true),
            sleepers: Mutex::new(Sleeper::new()),
            active: Mutex::new(Colo::new()),
        }
    }

    #[inline]
    pub fn notify(&self) {
        if self
            .notified
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let waker = self.sleepers.lock().unwrap().notify();
            if let Some(w) = waker {
                w.wake();
            }
        }
    }
}

#[derive(Debug)]
struct Ticker<'a> {
    state: &'a State,
    sleeping: AtomicUsize,
}

impl Ticker<'_> {
    fn new(state: &State) -> Ticker<'_> {
        Ticker {
            state,
            sleeping: AtomicUsize::new(0),
        }
    }

    fn sleep(&self, waker: &Waker) -> bool {
        let mut sleepers = self.state.sleepers.lock().unwrap();

        match self.sleeping.load(Ordering::SeqCst) {
            0 => self
                .sleeping
                .store(sleepers.insert(waker), Ordering::SeqCst),

            id => {
                if !sleepers.update(id, waker) {
                    return false;
                }
            }
        }

        self.state
            .notified
            .swap(sleepers.is_notified(), Ordering::SeqCst);

        true
    }

    fn wake(&self) {
        let id = self.sleeping.swap(0, Ordering::SeqCst);
        if id != 0 {
            let mut sleepers = self.state.sleepers.lock().unwrap();
            sleepers.remove(id);

            self.state
                .notified
                .swap(sleepers.is_notified(), Ordering::SeqCst);
        }
    }

    async fn runnable(&self) -> Runnable {
        self.runnable_with(|| self.state.queue.pop().ok()).await
    }

    async fn runnable_with(&self, mut search: impl FnMut() -> Option<Runnable>) -> Runnable {
        future::poll_fn(|cx| loop {
            match search() {
                None => {
                    if !self.sleep(cx.waker()) {
                        return Poll::Pending;
                    }
                }
                Some(r) => {
                    self.wake();

                    self.state.notify();

                    return Poll::Ready(r);
                }
            }
        })
        .await
    }
}

impl Drop for Ticker<'_> {
    fn drop(&mut self) {
        let id = self.sleeping.swap(0, Ordering::SeqCst);
        if id != 0 {
            let mut sleepers = self.state.sleepers.lock().unwrap();
            let notified = sleepers.remove(id);

            self.state
                .notified
                .swap(sleepers.is_notified(), Ordering::SeqCst);

            if notified {
                drop(sleepers);
                self.state.notify();
            }
        }
    }
}

#[derive(Debug)]
struct Runner<'a> {
    state: &'a State,
    ticker: Ticker<'a>,
    local: Arc<ConcurrentQueue<Runnable>>,
    ticks: AtomicUsize,
}

impl Runner<'_> {
    fn new(state: &State) -> Runner<'_> {
        let runner = Runner {
            state,
            ticker: Ticker::new(state),
            local: Arc::new(ConcurrentQueue::bounded(512)),
            ticks: AtomicUsize::new(0),
        };
        state
            .local_queues
            .write()
            .unwrap()
            .push(runner.local.clone());
        runner
    }

    async fn runnable(&self) -> Runnable {
        let runnable = self
            .ticker
            .runnable_with(|| {
                if let Ok(r) = self.local.pop() {
                    return Some(r);
                }

                if let Ok(r) = self.state.queue.pop() {
                    steal(&self.state.queue, &self.local);
                    return Some(r);
                }

                let local_queues = self.state.local_queues.read().unwrap();

                let n = local_queues.len();
                let start = rander::usize(..n);
                let iter = local_queues
                    .iter()
                    .chain(local_queues.iter())
                    .skip(start)
                    .take(n);

                let iter = iter.filter(|local| !Arc::ptr_eq(local, &self.local));

                for local in iter {
                    steal(local, &self.local);
                    if let Ok(r) = self.local.pop() {
                        return Some(r);
                    }
                }

                None
            })
            .await;

        let ticks = self.ticks.fetch_add(1, Ordering::SeqCst);

        if ticks % 64 == 0 {
            steal(&self.state.queue, &self.local);
        }

        runnable
    }
}

impl Drop for Runner<'_> {
    fn drop(&mut self) {
        self.state
            .local_queues
            .write()
            .unwrap()
            .retain(|local| !Arc::ptr_eq(local, &self.local));

        while let Ok(r) = self.local.pop() {
            r.schedule();
        }
    }
}

#[derive(Debug)]
pub struct Executor<'a> {
    state: once_cell::sync::OnceCell<Arc<State>>,
    _marker: PhantomData<std::cell::UnsafeCell<&'a ()>>,
}

unsafe impl Send for Executor<'_> {}
unsafe impl Sync for Executor<'_> {}

impl UnwindSafe for Executor<'_> {}
impl RefUnwindSafe for Executor<'_> {}

impl<'a> Executor<'a> {
    pub const fn new() -> Executor<'a> {
        Executor {
            state: once_cell::sync::OnceCell::new(),
            _marker: PhantomData,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.state().active.lock().unwrap().is_empty()
    }

    pub fn spawn<T: Send + 'a>(&self, future: impl Future<Output = T> + Send + 'a) -> Task<T> {
        let mut active = self.state().active.lock().unwrap();

        let index = active.next_vacant();
        let state = self.state().clone();
        let future = async move {
            let _guard = CallOnDrop(move || drop(state.active.lock().unwrap().remove(index)));
            future.await
        };

        let (runnable, task) = unsafe { task::spawn_unchecked(future, self.schedule()) };
        active.insert(runnable.waker());

        runnable.schedule();
        task
    }

    pub fn try_tick(&self) -> bool {
        match self.state().queue.pop() {
            Err(_) => false,
            Ok(runnable) => {
                self.state().notify();

                runnable.run();
                true
            }
        }
    }

    pub async fn tick(&self) {
        let state = self.state();
        let runnable = Ticker::new(state).runnable().await;
        runnable.run();
    }

    pub async fn run<T>(&self, future: impl Future<Output = T>) -> T {
        let runner = Runner::new(self.state());

        let run_forever = async {
            loop {
                for _ in 0..200 {
                    let runnable = runner.runnable().await;
                    runnable.run();
                }
                future::yield_now().await;
            }
        };

        future.or(run_forever).await
    }

    fn schedule(&self) -> impl Fn(Runnable) + Send + Sync + 'static {
        let state = self.state().clone();

        move |runnable| {
            state.queue.push(runnable).unwrap();
            state.notify();
        }
    }

    pub fn state(&self) -> &Arc<State> {
        self.state.get_or_init(|| Arc::new(State::new()))
    }
}

impl Drop for Executor<'_> {
    fn drop(&mut self) {
        if let Some(state) = self.state.get() {
            let mut active = state.active.lock().unwrap();
            for i in 0..active.capacity() {
                if let Some(w) = active.remove(i) {
                    w.wake();
                }
            }
            drop(active);

            while state.queue.pop().is_ok() {}
        }
    }
}

impl<'a> Default for Executor<'a> {
    fn default() -> Executor<'a> {
        Executor::new()
    }
}

fn steal<T>(src: &ConcurrentQueue<T>, dest: &ConcurrentQueue<T>) {
    let mut count = (src.len() + 1) / 2;

    if count > 0 {
        if let Some(cap) = dest.capacity() {
            count = count.min(cap - dest.len());
        }

        for _ in 0..count {
            if let Ok(t) = src.pop() {
                assert!(dest.push(t).is_ok());
            } else {
                break;
            }
        }
    }
}

struct CallOnDrop<F: Fn()>(F);

impl<F: Fn()> Drop for CallOnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

#[derive(Debug)]
pub struct MonoExecutor<'a> {
    reactor: once_cell::unsync::OnceCell<Arc<Reactor>>,
    inner: once_cell::unsync::OnceCell<Executor<'a>>,
    _marker: PhantomData<Rc<()>>,
}

impl UnwindSafe for MonoExecutor<'_> {}
impl RefUnwindSafe for MonoExecutor<'_> {}

impl<'a> MonoExecutor<'a> {
    pub const fn new() -> MonoExecutor<'a> {
        MonoExecutor {
            reactor: once_cell::unsync::OnceCell::new(),
            inner: once_cell::unsync::OnceCell::new(),
            _marker: PhantomData,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner().is_empty()
    }

    pub fn spawn<T: 'a>(&self, future: impl Future<Output = T> + 'a) -> Task<T> {
        let mut active = self.inner().state().active.lock().unwrap();

        let index = active.next_vacant();
        let state = self.inner().state().clone();
        let future = async move {
            let _guard = CallOnDrop(move || drop(state.active.lock().unwrap().remove(index)));
            future.await
        };

        let (runnable, task) =
            unsafe { crate::runtime::task::spawn_unchecked(future, self.schedule()) };
        active.insert(runnable.waker());

        runnable.schedule();
        task
    }

    pub fn try_tick(&self) -> bool {
        self.inner().try_tick()
    }

    pub async fn tick(&self) {
        self.inner().tick().await
    }

    pub async fn run<T>(&self, future: impl Future<Output = T>) -> T {
        self.inner().run(future).await
    }

    fn schedule(&self) -> impl Fn(Runnable) + Send + Sync + 'static {
        let state = self.inner().state().clone();

        move |runnable| {
            state.queue.push(runnable).unwrap();
            state.notify();
        }
    }

    fn inner(&self) -> &Executor<'a> {
        self.reactor.get_or_init(|| Arc::new(Reactor::new()));
        self.inner.get_or_init(|| Executor::new())
    }
}

impl<'a> Default for MonoExecutor<'a> {
    fn default() -> MonoExecutor<'a> {
        MonoExecutor::new()
    }
}
