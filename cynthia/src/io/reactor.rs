use once_cell::sync::Lazy;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};
use std::{io, mem, panic};

use crate::io::fd::TransportFd;
use crate::net::polling::{Event, Poller};
use crate::platform::queue::ConcurrentQueue;
use crate::utils::Colo;

const READ: usize = 0;
const WRITE: usize = 1;

enum TimerOp {
    Insert(Instant, usize, Waker),
    Remove(Instant, usize),
}

struct CallOnDrop<F: Fn()>(F);

impl<F: Fn()> Drop for CallOnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

#[derive(Debug, Default)]
struct Private {
    data: usize,
    tick: usize,
    ticks: Option<(usize, usize)>,
    waker: Option<Waker>,
    wakers: Colo<Option<Waker>>,
}

impl Private {
    fn is_empty(&self) -> bool {
        self.waker.is_none() && self.wakers.iter().all(|(_, opt)| opt.is_none())
    }

    fn drain_into(&mut self, dst: &mut Vec<Waker>) {
        if let Some(w) = self.waker.take() {
            dst.push(w);
        }
        for (_, opt) in self.wakers.iter_mut() {
            if let Some(w) = opt.take() {
                dst.push(w);
            }
        }
    }
}

#[derive(Debug)]
pub struct Source {
    pub raw: TransportFd,
    key: usize,
    state: Mutex<[Private; 2]>,
}

impl Source {
    pub fn poll_readable(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_ready(READ, cx)
    }

    pub fn poll_writable(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_ready(WRITE, cx)
    }

    fn poll_ready(&self, dir: usize, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut state = self.state.lock().unwrap();
        if let Some((a, b)) = state[dir].ticks {
            if state[dir].tick != a && state[dir].tick != b {
                state[dir].ticks = None;
                return Poll::Ready(Ok(()));
            }
        }

        let was_empty = state[dir].is_empty();

        if let Some(w) = state[dir].waker.take() {
            if w.will_wake(cx.waker()) {
                state[dir].waker = Some(w);
                return Poll::Pending;
            }
            panic::catch_unwind(|| w.wake()).ok();
        }
        state[dir].waker = Some(cx.waker().clone());
        state[dir].ticks = Some((Reactor::get().ticker(), state[dir].tick));

        if was_empty {
            Reactor::get().poller.modify(
                self.raw,
                Event {
                    key: self.key,
                    readable: !state[READ].is_empty(),
                    writable: !state[WRITE].is_empty(),
                },
            )?;
        }

        Poll::Pending
    }

    pub async fn readable(&self) -> io::Result<()> {
        self.ready(READ).await?;
        Ok(())
    }

    pub async fn writable(&self) -> io::Result<()> {
        self.ready(WRITE).await?;
        Ok(())
    }

    async fn ready(&self, dir: usize) -> io::Result<()> {
        use crate::future::future;

        let mut ticks = None;
        let mut index = None;
        let mut _guard = None;

        future::poll_fn(|cx| {
            let mut state = self.state.lock().unwrap();

            if let Some((a, b)) = ticks {
                if state[dir].tick != a && state[dir].tick != b {
                    return Poll::Ready(Ok(()));
                }
            }

            let was_empty = state[dir].is_empty();

            let i = match index {
                Some(i) => i,
                None => {
                    let i = state[dir].wakers.insert(None);
                    _guard = Some(CallOnDrop(move || {
                        let mut state = self.state.lock().unwrap();
                        state[dir].wakers.remove(i);
                    }));
                    index = Some(i);
                    ticks = Some((Reactor::get().ticker(), state[dir].tick));
                    i
                }
            };
            state[dir].wakers[i] = Some(cx.waker().clone());

            if was_empty {
                Reactor::get().poller.modify(
                    self.raw,
                    Event {
                        key: self.key,
                        readable: !state[READ].is_empty(),
                        writable: !state[WRITE].is_empty(),
                    },
                )?;
            }

            Poll::Pending
        })
        .await
    }
}

#[derive(Debug)]
pub struct Reactor {
    poller: Poller,
    ticker: AtomicUsize,
    sources: Mutex<Colo<Arc<Source>>>,
    events: Mutex<Vec<Event>>,
    timers: Mutex<BTreeMap<(Instant, usize), Waker>>,
    timer_ops: ConcurrentQueue<TimerOp>,
}

impl Reactor {
    pub fn new() -> Reactor {
        Reactor {
            poller: Poller::new().expect("I/O event poller initialize failed"),
            ticker: AtomicUsize::new(0),
            sources: Mutex::new(Colo::new()),
            events: Mutex::new(Vec::new()),
            timers: Mutex::new(BTreeMap::new()),
            timer_ops: ConcurrentQueue::bounded(1000),
        }
    }

    pub fn get() -> &'static Reactor {
        static REACTOR: Lazy<Reactor> = Lazy::new(|| {
            crate::io::worker::init();
            Reactor {
                poller: Poller::new().expect("I/O event poller initialize failed"),
                ticker: AtomicUsize::new(0),
                sources: Mutex::new(Colo::new()),
                events: Mutex::new(Vec::new()),
                timers: Mutex::new(BTreeMap::new()),
                timer_ops: ConcurrentQueue::bounded(1000),
            }
        });
        &REACTOR
    }

    pub fn ticker(&self) -> usize {
        self.ticker.load(Ordering::SeqCst)
    }

    pub fn add_io(&self, raw: TransportFd) -> io::Result<Arc<Source>> {
        let source = {
            let mut sources = self.sources.lock().unwrap();
            let key = sources.next_vacant();
            let source = Arc::new(Source {
                raw,
                key,
                state: Default::default(),
            });
            sources.insert(source.clone());
            source
        };

        if let Err(err) = self.poller.add(raw, Event::none(source.key)) {
            let mut sources = self.sources.lock().unwrap();
            sources.remove(source.key);
            return Err(err);
        }

        Ok(source)
    }

    pub fn remove_io(&self, source: &Source) -> io::Result<()> {
        let mut sources = self.sources.lock().unwrap();
        sources.remove(source.key);
        self.poller.delete(source.raw)
    }

    pub fn add_timer(&self, when: Instant, waker: &Waker) -> usize {
        static TIMER_GLOBAL_ID: AtomicUsize = AtomicUsize::new(1);
        let id = TIMER_GLOBAL_ID.fetch_add(1, Ordering::Relaxed);

        while self
            .timer_ops
            .push(TimerOp::Insert(when, id, waker.clone()))
            .is_err()
        {
            let mut timers = self.timers.lock().unwrap();
            self.process_timer_ops(&mut timers);
        }

        self.notify();

        id
    }

    pub fn remove_timer(&self, when: Instant, id: usize) {
        while self.timer_ops.push(TimerOp::Remove(when, id)).is_err() {
            let mut timers = self.timers.lock().unwrap();
            self.process_timer_ops(&mut timers);
        }
    }

    pub fn notify(&self) {
        self.poller.notify().expect("failed to notify reactor");
    }

    pub fn lock(&self) -> LockedReactor<'_> {
        let reactor = self;
        let events = self.events.lock().unwrap();
        LockedReactor { reactor, events }
    }

    pub fn try_lock(&self) -> Option<LockedReactor<'_>> {
        self.events.try_lock().ok().map(|events| {
            let reactor = self;
            LockedReactor { reactor, events }
        })
    }

    fn process_timers(&self, wakers: &mut Vec<Waker>) -> Option<Duration> {
        let mut timers = self.timers.lock().unwrap();
        self.process_timer_ops(&mut timers);

        let now = Instant::now();

        let pending = timers.split_off(&(now, 0));
        let ready = mem::replace(&mut *timers, pending);

        let dur = if ready.is_empty() {
            timers
                .keys()
                .next()
                .map(|(when, _)| when.saturating_duration_since(now))
        } else {
            Some(Duration::from_secs(0))
        };

        drop(timers);

        for (_, waker) in ready {
            wakers.push(waker);
        }

        dur
    }

    fn process_timer_ops(&self, timers: &mut MutexGuard<'_, BTreeMap<(Instant, usize), Waker>>) {
        for _ in 0..self.timer_ops.capacity().unwrap() {
            match self.timer_ops.pop() {
                Ok(TimerOp::Insert(when, id, waker)) => {
                    timers.insert((when, id), waker);
                }
                Ok(TimerOp::Remove(when, id)) => {
                    timers.remove(&(when, id));
                }
                Err(_) => break,
            }
        }
    }
}

pub struct LockedReactor<'a> {
    reactor: &'a Reactor,
    events: MutexGuard<'a, Vec<Event>>,
}

impl LockedReactor<'_> {
    pub fn react(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let mut wakers = Vec::new();
        let next_timer = self.reactor.process_timers(&mut wakers);

        let timeout = match (next_timer, timeout) {
            (None, None) => None,
            (Some(t), None) | (None, Some(t)) => Some(t),
            (Some(a), Some(b)) => Some(a.min(b)),
        };

        let tick = self
            .reactor
            .ticker
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);

        self.events.clear();

        let res = match self.reactor.poller.wait(&mut self.events, timeout) {
            Ok(0) => {
                if timeout != Some(Duration::from_secs(0)) {
                    self.reactor.process_timers(&mut wakers);
                }
                Ok(())
            }

            Ok(_) => {
                let sources = self.reactor.sources.lock().unwrap();
                for ev in self.events.iter() {
                    if let Some(source) = sources.get(ev.key) {
                        let mut state = source.state.lock().unwrap();
                        for &(dir, emitted) in &[(WRITE, ev.writable), (READ, ev.readable)] {
                            if emitted {
                                state[dir].tick = tick;
                                state[dir].drain_into(&mut wakers);
                            }
                        }

                        if !state[READ].is_empty() || !state[WRITE].is_empty() {
                            self.reactor.poller.modify(
                                source.raw,
                                Event {
                                    key: source.key,
                                    readable: !state[READ].is_empty(),
                                    writable: !state[WRITE].is_empty(),
                                },
                            )?;
                        }
                    }
                }

                Ok(())
            }

            Err(err) if err.kind() == io::ErrorKind::Interrupted => Ok(()),

            Err(err) => Err(err),
        };

        for waker in wakers {
            panic::catch_unwind(|| waker.wake()).ok();
        }

        res
    }
}
