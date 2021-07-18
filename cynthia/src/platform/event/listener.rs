use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::future::Future;
use std::mem::{self, ManuallyDrop};
use std::ops::{Deref, DerefMut};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::sync::atomic::{self, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};
use std::usize;

struct Inner {
    notified: AtomicUsize,
    list: Mutex<List>,
    cache: UnsafeCell<Entry>,
}

impl Inner {
    fn lock(&self) -> ListGuard<'_> {
        ListGuard {
            inner: self,
            guard: self.list.lock().unwrap(),
        }
    }

    #[inline(always)]
    fn cache_ptr(&self) -> NonNull<Entry> {
        unsafe { NonNull::new_unchecked(self.cache.get()) }
    }
}

pub struct Event {
    inner: AtomicPtr<Inner>,
}

unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl UnwindSafe for Event {}
impl RefUnwindSafe for Event {}

impl Event {
    #[inline]
    pub const fn new() -> Event {
        Event {
            inner: AtomicPtr::new(ptr::null_mut()),
        }
    }

    #[cold]
    pub fn listen(&self) -> EventListener {
        let inner = self.inner();
        let listener = EventListener {
            inner: unsafe { Arc::clone(&ManuallyDrop::new(Arc::from_raw(inner))) },
            entry: Some(inner.lock().insert(inner.cache_ptr())),
        };

        full_fence();
        listener
    }

    #[inline]
    pub fn notify(&self, n: usize) {
        full_fence();

        if let Some(inner) = self.try_inner() {
            if inner.notified.load(Ordering::Acquire) < n {
                inner.lock().notify(n);
            }
        }
    }

    #[inline]
    pub fn notify_relaxed(&self, n: usize) {
        if let Some(inner) = self.try_inner() {
            if inner.notified.load(Ordering::Acquire) < n {
                inner.lock().notify(n);
            }
        }
    }

    #[inline]
    pub fn notify_additional(&self, n: usize) {
        full_fence();

        if let Some(inner) = self.try_inner() {
            if inner.notified.load(Ordering::Acquire) < usize::MAX {
                inner.lock().notify_additional(n);
            }
        }
    }

    #[inline]
    pub fn notify_additional_relaxed(&self, n: usize) {
        if let Some(inner) = self.try_inner() {
            if inner.notified.load(Ordering::Acquire) < usize::MAX {
                inner.lock().notify_additional(n);
            }
        }
    }

    #[inline]
    fn try_inner(&self) -> Option<&Inner> {
        let inner = self.inner.load(Ordering::Acquire);
        unsafe { inner.as_ref() }
    }

    fn inner(&self) -> &Inner {
        let mut inner = self.inner.load(Ordering::Acquire);

        if inner.is_null() {
            let new = Arc::new(Inner {
                notified: AtomicUsize::new(usize::MAX),
                list: std::sync::Mutex::new(List {
                    head: None,
                    tail: None,
                    start: None,
                    len: 0,
                    notified: 0,
                    cache_used: false,
                }),
                cache: UnsafeCell::new(Entry {
                    state: Cell::new(State::Created),
                    prev: Cell::new(None),
                    next: Cell::new(None),
                }),
            });
            let new = Arc::into_raw(new) as *mut Inner;

            inner = self
                .inner
                .compare_exchange(inner, new, Ordering::AcqRel, Ordering::Acquire)
                .unwrap_or_else(|x| x);

            if inner.is_null() {
                inner = new;
            } else {
                unsafe {
                    drop(Arc::from_raw(new));
                }
            }
        }

        unsafe { &*inner }
    }
}

impl Drop for Event {
    #[inline]
    fn drop(&mut self) {
        let inner: *mut Inner = *self.inner.get_mut();

        if !inner.is_null() {
            unsafe {
                drop(Arc::from_raw(inner));
            }
        }
    }
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("Event { .. }")
    }
}

impl Default for Event {
    fn default() -> Event {
        Event::new()
    }
}

pub struct EventListener {
    inner: Arc<Inner>,
    entry: Option<NonNull<Entry>>,
}

unsafe impl Send for EventListener {}
unsafe impl Sync for EventListener {}

impl UnwindSafe for EventListener {}
impl RefUnwindSafe for EventListener {}

impl EventListener {
    pub fn wait(self) {
        self.wait_internal(None);
    }

    pub fn wait_timeout(self, timeout: Duration) -> bool {
        self.wait_internal(Some(Instant::now() + timeout))
    }

    pub fn wait_deadline(self, deadline: Instant) -> bool {
        self.wait_internal(Some(deadline))
    }

    pub fn discard(mut self) -> bool {
        if let Some(entry) = self.entry.take() {
            let mut list = self.inner.lock();
            if let State::Notified(_) = list.remove(entry, self.inner.cache_ptr()) {
                return true;
            }
        }
        false
    }

    #[inline]
    pub fn listens_to(&self, event: &Event) -> bool {
        ptr::eq::<Inner>(&*self.inner, event.inner.load(Ordering::Acquire))
    }

    pub fn same_event(&self, other: &EventListener) -> bool {
        ptr::eq::<Inner>(&*self.inner, &*other.inner)
    }

    fn wait_internal(mut self, deadline: Option<Instant>) -> bool {
        let entry = match self.entry.take() {
            None => unreachable!("cannot wait twice on an `EventListener`"),
            Some(entry) => entry,
        };

        {
            let mut list = self.inner.lock();
            let e = unsafe { entry.as_ref() };

            match e.state.replace(State::Notified(false)) {
                State::Notified(_) => {
                    list.remove(entry, self.inner.cache_ptr());
                    return true;
                }
                _ => e.state.set(State::Waiting(thread::current())),
            }
        }

        loop {
            match deadline {
                None => thread::park(),

                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return self
                            .inner
                            .lock()
                            .remove(entry, self.inner.cache_ptr())
                            .is_notified();
                    }

                    thread::park_timeout(deadline - now);
                }
            }

            let mut list = self.inner.lock();
            let e = unsafe { entry.as_ref() };

            match e.state.replace(State::Notified(false)) {
                State::Notified(_) => {
                    list.remove(entry, self.inner.cache_ptr());
                    return true;
                }
                state => e.state.set(state),
            }
        }
    }
}

impl fmt::Debug for EventListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("EventListener { .. }")
    }
}

impl Future for EventListener {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut list = self.inner.lock();

        let entry = match self.entry {
            None => unreachable!("cannot poll a completed `EventListener` future"),
            Some(entry) => entry,
        };
        let state = unsafe { &entry.as_ref().state };

        match state.replace(State::Notified(false)) {
            State::Notified(_) => {
                list.remove(entry, self.inner.cache_ptr());
                drop(list);
                self.entry = None;
                return Poll::Ready(());
            }
            State::Created => {
                state.set(State::Polling(cx.waker().clone()));
            }
            State::Polling(w) => {
                if w.will_wake(cx.waker()) {
                    state.set(State::Polling(w));
                } else {
                    state.set(State::Polling(cx.waker().clone()));
                }
            }
            State::Waiting(_) => {
                unreachable!("cannot poll and wait on `EventListener` at the same time")
            }
        }

        Poll::Pending
    }
}

impl Drop for EventListener {
    fn drop(&mut self) {
        if let Some(entry) = self.entry.take() {
            let mut list = self.inner.lock();

            if let State::Notified(additional) = list.remove(entry, self.inner.cache_ptr()) {
                if additional {
                    list.notify_additional(1);
                } else {
                    list.notify(1);
                }
            }
        }
    }
}

struct ListGuard<'a> {
    inner: &'a Inner,
    guard: MutexGuard<'a, List>,
}

impl Drop for ListGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        let list = &mut **self;

        let notified = if list.notified < list.len {
            list.notified
        } else {
            usize::MAX
        };
        self.inner.notified.store(notified, Ordering::Release);
    }
}

impl Deref for ListGuard<'_> {
    type Target = List;

    #[inline]
    fn deref(&self) -> &List {
        &*self.guard
    }
}

impl DerefMut for ListGuard<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut List {
        &mut *self.guard
    }
}

enum State {
    Created,
    Notified(bool),
    Polling(Waker),
    Waiting(Thread),
}

impl State {
    #[inline]
    fn is_notified(&self) -> bool {
        match self {
            State::Notified(_) => true,
            State::Created | State::Polling(_) | State::Waiting(_) => false,
        }
    }
}

struct Entry {
    state: Cell<State>,
    prev: Cell<Option<NonNull<Entry>>>,
    next: Cell<Option<NonNull<Entry>>>,
}

#[allow(dead_code)]
struct List {
    head: Option<NonNull<Entry>>,
    tail: Option<NonNull<Entry>>,
    start: Option<NonNull<Entry>>,
    len: usize,
    notified: usize,
    cache_used: bool,
}

impl List {
    fn insert(&mut self, cache: NonNull<Entry>) -> NonNull<Entry> {
        unsafe {
            let entry = Entry {
                state: Cell::new(State::Created),
                prev: Cell::new(self.tail),
                next: Cell::new(None),
            };

            let entry = if self.cache_used {
                NonNull::new_unchecked(Box::into_raw(Box::new(entry)))
            } else {
                self.cache_used = true;
                cache.as_ptr().write(entry);
                cache
            };

            match mem::replace(&mut self.tail, Some(entry)) {
                None => self.head = Some(entry),
                Some(t) => t.as_ref().next.set(Some(entry)),
            }

            if self.start.is_none() {
                self.start = self.tail;
            }

            self.len += 1;

            entry
        }
    }

    fn remove(&mut self, entry: NonNull<Entry>, cache: NonNull<Entry>) -> State {
        unsafe {
            let prev = entry.as_ref().prev.get();
            let next = entry.as_ref().next.get();

            match prev {
                None => self.head = next,
                Some(p) => p.as_ref().next.set(next),
            }

            match next {
                None => self.tail = prev,
                Some(n) => n.as_ref().prev.set(prev),
            }

            if self.start == Some(entry) {
                self.start = next;
            }

            let state = if ptr::eq(entry.as_ptr(), cache.as_ptr()) {
                self.cache_used = false;
                entry.as_ref().state.replace(State::Created)
            } else {
                Box::from_raw(entry.as_ptr()).state.into_inner()
            };

            if state.is_notified() {
                self.notified -= 1;
            }
            self.len -= 1;

            state
        }
    }

    #[cold]
    fn notify(&mut self, mut n: usize) {
        if n <= self.notified {
            return;
        }
        n -= self.notified;

        while n > 0 {
            n -= 1;

            match self.start {
                None => break,
                Some(e) => {
                    let e = unsafe { e.as_ref() };
                    self.start = e.next.get();

                    match e.state.replace(State::Notified(false)) {
                        State::Notified(_) => {}
                        State::Created => {}
                        State::Polling(w) => w.wake(),
                        State::Waiting(t) => t.unpark(),
                    }

                    self.notified += 1;
                }
            }
        }
    }

    #[cold]
    fn notify_additional(&mut self, mut n: usize) {
        while n > 0 {
            n -= 1;

            match self.start {
                None => break,
                Some(e) => {
                    let e = unsafe { e.as_ref() };
                    self.start = e.next.get();

                    match e.state.replace(State::Notified(true)) {
                        State::Notified(_) => {}
                        State::Created => {}
                        State::Polling(w) => w.wake(),
                        State::Waiting(t) => t.unpark(),
                    }

                    self.notified += 1;
                }
            }
        }
    }
}

#[inline]
fn full_fence() {
    if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        let a = AtomicUsize::new(0);
        let _ = a.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst);
    } else {
        atomic::fence(Ordering::SeqCst);
    }
}
