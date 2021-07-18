use std::any::Any;
use std::collections::VecDeque;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::pin::Pin;
use std::sync::atomic::{self, AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Duration;
use std::{fmt, mem, panic, slice, thread};
use once_cell::sync::Lazy;

use crate::platform::atomic_waker::AtomicWaker;
use crate::platform::channel::{bounded, Receiver};
use crate::runtime::task::{self, Runnable, Task};
use crate::utils::rander;
use crate::{
    future::{future, prelude::*},
    ready,
};

enum State<T> {
    Idle(Option<Box<T>>),
    WithMut(Task<Box<T>>),
    Streaming(Option<Box<dyn Any + Send + Sync>>, Task<Box<T>>),
    Reading(Option<Reader>, Task<(io::Result<()>, Box<T>)>),
    Writing(Option<Writer>, Task<(io::Result<()>, Box<T>)>),
    Seeking(Task<(SeekFrom, io::Result<u64>, Box<T>)>),
}

pub struct Unblock<T> {
    state: State<T>,
    cap: Option<usize>,
}

impl<T> Unblock<T> {
    pub fn new(io: T) -> Unblock<T> {
        Unblock {
            state: State::Idle(Some(Box::new(io))),
            cap: None,
        }
    }

    pub fn with_capacity(cap: usize, io: T) -> Unblock<T> {
        Unblock {
            state: State::Idle(Some(Box::new(io))),
            cap: Some(cap),
        }
    }

    pub async fn get_mut(&mut self) -> &mut T {
        future::poll_fn(|cx| self.poll_stop(cx)).await.ok();

        match &mut self.state {
            State::Idle(t) => t.as_mut().expect("inner value was taken out"),
            State::WithMut(..)
            | State::Streaming(..)
            | State::Reading(..)
            | State::Writing(..)
            | State::Seeking(..) => {
                unreachable!("when stopped, the state machine must be in idle state");
            }
        }
    }

    pub async fn with_mut<R, F>(&mut self, op: F) -> R
    where
        F: FnOnce(&mut T) -> R + Send + 'static,
        R: Send + 'static,
        T: Send + 'static,
    {
        future::poll_fn(|cx| self.poll_stop(cx)).await.ok();

        let mut t = match &mut self.state {
            State::Idle(t) => t.take().expect("inner value was taken out"),
            State::WithMut(..)
            | State::Streaming(..)
            | State::Reading(..)
            | State::Writing(..)
            | State::Seeking(..) => {
                unreachable!("when stopped, the state machine must be in idle state");
            }
        };

        let (sender, receiver) = bounded(1);
        let task = Executor::spawn(async move {
            sender.try_send(op(&mut t)).ok();
            t
        });
        self.state = State::WithMut(task);

        receiver
            .recv()
            .await
            .expect("`Unblock::with_mut()` operation has panicked")
    }

    pub async fn into_inner(self) -> T {
        let mut this = self;

        future::poll_fn(|cx| this.poll_stop(cx)).await.ok();

        match &mut this.state {
            State::Idle(t) => *t.take().expect("inner value was taken out"),
            State::WithMut(..)
            | State::Streaming(..)
            | State::Reading(..)
            | State::Writing(..)
            | State::Seeking(..) => {
                unreachable!("when stopped, the state machine must be in idle state");
            }
        }
    }

    fn poll_stop(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            match &mut self.state {
                State::Idle(_) => return Poll::Ready(Ok(())),

                State::WithMut(task) => {
                    let io = ready!(Pin::new(task).poll(cx));
                    self.state = State::Idle(Some(io));
                }

                State::Streaming(any, task) => {
                    any.take();

                    let iter = crate::ready!(Pin::new(task).poll(cx));
                    self.state = State::Idle(Some(iter));
                }

                State::Reading(reader, task) => {
                    reader.take();

                    let (res, io) = crate::ready!(Pin::new(task).poll(cx));
                    self.state = State::Idle(Some(io));
                    res?;
                }

                State::Writing(writer, task) => {
                    writer.take();

                    let (res, io) = crate::ready!(Pin::new(task).poll(cx));
                    self.state = State::Idle(Some(io));
                    res?;
                }

                State::Seeking(task) => {
                    let (_, res, io) = crate::ready!(Pin::new(task).poll(cx));
                    self.state = State::Idle(Some(io));
                    res?;
                }
            }
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Unblock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Closed;
        impl fmt::Debug for Closed {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("<closed>")
            }
        }

        struct Blocked;
        impl fmt::Debug for Blocked {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("<blocked>")
            }
        }

        match &self.state {
            State::Idle(None) => f.debug_struct("Unblock").field("io", &Closed).finish(),
            State::Idle(Some(io)) => {
                let io: &T = &*io;
                f.debug_struct("Unblock").field("io", io).finish()
            }
            State::WithMut(..)
            | State::Streaming(..)
            | State::Reading(..)
            | State::Writing(..)
            | State::Seeking(..) => f.debug_struct("Unblock").field("io", &Blocked).finish(),
        }
    }
}

impl<T: Iterator + Send + 'static> Stream for Unblock<T>
where
    T::Item: Send + 'static,
{
    type Item = T::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T::Item>> {
        loop {
            match &mut self.state {
                State::WithMut(..)
                | State::Streaming(None, _)
                | State::Reading(..)
                | State::Writing(..)
                | State::Seeking(..) => {
                    crate::ready!(self.poll_stop(cx)).ok();
                }

                State::Idle(iter) => {
                    let mut iter = iter.take().expect("inner iterator was taken out");

                    let (sender, receiver) = bounded(self.cap.unwrap_or(8 * 1024));

                    let task = Executor::spawn(async move {
                        for item in &mut iter {
                            if sender.send(item).await.is_err() {
                                break;
                            }
                        }
                        iter
                    });

                    self.state = State::Streaming(Some(Box::new(receiver)), task);
                }

                State::Streaming(Some(any), task) => {
                    let receiver = any.downcast_mut::<Receiver<T::Item>>().unwrap();

                    let opt = crate::ready!(Pin::new(receiver).poll_next(cx));

                    if opt.is_none() {
                        let iter = crate::ready!(Pin::new(task).poll(cx));
                        self.state = State::Idle(Some(iter));
                    }

                    return Poll::Ready(opt);
                }
            }
        }
    }
}

impl<T: Read + Send + 'static> AsyncRead for Unblock<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            match &mut self.state {
                State::WithMut(..)
                | State::Reading(None, _)
                | State::Streaming(..)
                | State::Writing(..)
                | State::Seeking(..) => {
                    crate::ready!(self.poll_stop(cx))?;
                }

                State::Idle(io) => {
                    let mut io = io.take().expect("inner value was taken out");

                    let (reader, mut writer) = pipe(self.cap.unwrap_or(8 * 1024 * 1024));

                    let task = Executor::spawn(async move {
                        loop {
                            match future::poll_fn(|cx| writer.fill(cx, &mut io)).await {
                                Ok(0) => return (Ok(()), io),
                                Ok(_) => {}
                                Err(err) => return (Err(err), io),
                            }
                        }
                    });

                    self.state = State::Reading(Some(reader), task);
                }

                State::Reading(Some(reader), task) => {
                    let n = crate::ready!(reader.drain(cx, buf))?;

                    if n == 0 {
                        let (res, io) = crate::ready!(Pin::new(task).poll(cx));
                        self.state = State::Idle(Some(io));
                        res?;
                    }

                    return Poll::Ready(Ok(n));
                }
            }
        }
    }
}

impl<T: Write + Send + 'static> AsyncWrite for Unblock<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            match &mut self.state {
                State::WithMut(..)
                | State::Writing(None, _)
                | State::Streaming(..)
                | State::Reading(..)
                | State::Seeking(..) => {
                    crate::ready!(self.poll_stop(cx))?;
                }

                State::Idle(io) => {
                    let mut io = io.take().expect("inner value was taken out");

                    let (mut reader, writer) = pipe(self.cap.unwrap_or(8 * 1024 * 1024));

                    let task = Executor::spawn(async move {
                        loop {
                            match future::poll_fn(|cx| reader.drain(cx, &mut io)).await {
                                Ok(0) => return (io.flush(), io),
                                Ok(_) => {}
                                Err(err) => {
                                    io.flush().ok();
                                    return (Err(err), io);
                                }
                            }
                        }
                    });

                    self.state = State::Writing(Some(writer), task);
                }

                State::Writing(Some(writer), _) => return writer.fill(cx, buf),
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            match &mut self.state {
                State::WithMut(..)
                | State::Streaming(..)
                | State::Writing(..)
                | State::Reading(..)
                | State::Seeking(..) => {
                    crate::ready!(self.poll_stop(cx))?;
                }

                State::Idle(_) => return Poll::Ready(Ok(())),
            }
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        crate::ready!(Pin::new(&mut self).poll_flush(cx))?;

        self.state = State::Idle(None);
        Poll::Ready(Ok(()))
    }
}

impl<T: Seek + Send + 'static> AsyncSeek for Unblock<T> {
    fn poll_seek(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        pos: SeekFrom,
    ) -> Poll<io::Result<u64>> {
        loop {
            match &mut self.state {
                State::WithMut(..)
                | State::Streaming(..)
                | State::Reading(..)
                | State::Writing(..) => {
                    crate::ready!(self.poll_stop(cx))?;
                }

                State::Idle(io) => {
                    let mut io = io.take().expect("inner value was taken out");

                    let task = Executor::spawn(async move {
                        let res = io.seek(pos);
                        (pos, res, io)
                    });
                    self.state = State::Seeking(task);
                }

                State::Seeking(task) => {
                    let (original_pos, res, io) = crate::ready!(Pin::new(task).poll(cx));
                    self.state = State::Idle(Some(io));
                    let current = res?;

                    if original_pos == pos {
                        return Poll::Ready(Ok(current));
                    }
                }
            }
        }
    }
}

static EXECUTOR: Lazy<Executor> = Lazy::new(|| Executor {
    inner: Mutex::new(Inner {
        idle_count: 0,
        thread_count: 0,
        queue: VecDeque::new(),
    }),
    cvar: Condvar::new(),
});

struct Executor {
    inner: Mutex<Inner>,
    cvar: Condvar,
}

struct Inner {
    idle_count: usize,
    thread_count: usize,
    queue: VecDeque<Runnable>,
}

impl Executor {
    fn spawn<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> Task<T> {
        let (runnable, task) = task::spawn(future, |r| EXECUTOR.schedule(r));
        runnable.schedule();
        task
    }

    fn cycle_entry(&'static self) {
        let mut inner = self.inner.lock().unwrap();
        loop {
            inner.idle_count -= 1;

            while let Some(runnable) = inner.queue.pop_front() {
                self.grow_pool(inner);

                panic::catch_unwind(|| runnable.run()).ok();

                inner = self.inner.lock().unwrap();
            }

            inner.idle_count += 1;

            let timeout = Duration::from_millis(100);
            let (lock, res) = self.cvar.wait_timeout(inner, timeout).unwrap();
            inner = lock;

            if res.timed_out() && inner.queue.is_empty() {
                inner.idle_count -= 1;
                inner.thread_count -= 1;
                break;
            }
        }
    }

    fn schedule(&'static self, runnable: Runnable) {
        let mut inner = self.inner.lock().unwrap();
        inner.queue.push_back(runnable);

        self.cvar.notify_one();
        self.grow_pool(inner);
    }

    fn grow_pool(&'static self, mut inner: MutexGuard<'static, Inner>) {
        while inner.queue.len() > inner.idle_count * 5 && inner.thread_count < 500 {
            inner.idle_count += 1;
            inner.thread_count += 1;

            self.cvar.notify_all();

            static ID: AtomicUsize = AtomicUsize::new(1);
            let id = ID.fetch_add(1, Ordering::Relaxed);

            thread::Builder::new()
                .name(format!("cyn-blocking-{}", id))
                .spawn(move || self.cycle_entry())
                .unwrap();
        }
    }
}

struct Reader {
    inner: Arc<Pipe>,
    head: usize,
    tail: usize,
}

struct Writer {
    inner: Arc<Pipe>,
    head: usize,
    tail: usize,
    zeroed_until: usize,
}

unsafe impl Send for Reader {}
unsafe impl Send for Writer {}

struct Pipe {
    head: AtomicUsize,
    tail: AtomicUsize,
    reader: AtomicWaker,
    writer: AtomicWaker,
    closed: AtomicBool,
    buffer: *mut u8,
    cap: usize,
}

pub async fn unblock<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Executor::spawn(async move { f() }).await
}

fn pipe(cap: usize) -> (Reader, Writer) {
    assert!(cap > 0, "capacity must be positive");
    assert!(cap.checked_mul(2).is_some(), "capacity is too large");

    let mut v = Vec::with_capacity(cap);
    let buffer = v.as_mut_ptr();
    mem::forget(v);

    let inner = Arc::new(Pipe {
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        reader: AtomicWaker::new(),
        writer: AtomicWaker::new(),
        closed: AtomicBool::new(false),
        buffer,
        cap,
    });

    let r = Reader {
        inner: inner.clone(),
        head: 0,
        tail: 0,
    };

    let w = Writer {
        inner,
        head: 0,
        tail: 0,
        zeroed_until: 0,
    };

    (r, w)
}

unsafe impl Sync for Pipe {}
unsafe impl Send for Pipe {}

impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            Vec::from_raw_parts(self.buffer, 0, self.cap);
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.inner.writer.wake();
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.inner.reader.wake();
    }
}

impl Reader {
    fn drain(&mut self, cx: &mut Context<'_>, mut dest: impl Write) -> Poll<io::Result<usize>> {
        let cap = self.inner.cap;

        let distance = |a: usize, b: usize| {
            if a <= b {
                b - a
            } else {
                2 * cap - (a - b)
            }
        };

        if distance(self.head, self.tail) == 0 {
            self.tail = self.inner.tail.load(Ordering::Acquire);

            if distance(self.head, self.tail) == 0 {
                self.inner.reader.register(cx.waker());
                atomic::fence(Ordering::SeqCst);

                self.tail = self.inner.tail.load(Ordering::Acquire);

                if distance(self.head, self.tail) == 0 {
                    if self.inner.closed.load(Ordering::Relaxed) {
                        return Poll::Ready(Ok(0));
                    } else {
                        return Poll::Pending;
                    }
                }
            }
        }

        self.inner.reader.take();

        crate::ready!(maybe_yield(cx));

        let real_index = |i: usize| {
            if i < cap {
                i
            } else {
                i - cap
            }
        };

        let mut count = 0;

        loop {
            let n = (128 * 1024)
                .min(distance(self.head, self.tail))
                .min(cap - real_index(self.head));

            let pipe_slice =
                unsafe { slice::from_raw_parts(self.inner.buffer.add(real_index(self.head)), n) };

            let n = dest.write(pipe_slice)?;
            count += n;

            if n == 0 {
                return Poll::Ready(Ok(count));
            }

            if self.head + n < 2 * cap {
                self.head += n;
            } else {
                self.head = 0;
            }

            self.inner.head.store(self.head, Ordering::Release);

            self.inner.writer.wake();
        }
    }
}

impl Writer {
    fn fill(&mut self, cx: &mut Context<'_>, mut src: impl Read) -> Poll<io::Result<usize>> {
        if self.inner.closed.load(Ordering::Relaxed) {
            return Poll::Ready(Ok(0));
        }

        let cap = self.inner.cap;
        let distance = |a: usize, b: usize| {
            if a <= b {
                b - a
            } else {
                2 * cap - (a - b)
            }
        };

        if distance(self.head, self.tail) == cap {
            self.head = self.inner.head.load(Ordering::Acquire);

            if distance(self.head, self.tail) == cap {
                self.inner.writer.register(cx.waker());
                atomic::fence(Ordering::SeqCst);

                self.head = self.inner.head.load(Ordering::Acquire);

                if distance(self.head, self.tail) == cap {
                    if self.inner.closed.load(Ordering::Relaxed) {
                        return Poll::Ready(Ok(0));
                    } else {
                        return Poll::Pending;
                    }
                }
            }
        }

        self.inner.writer.take();

        crate::ready!(maybe_yield(cx));

        let real_index = |i: usize| {
            if i < cap {
                i
            } else {
                i - cap
            }
        };

        let mut count = 0;

        loop {
            let n = (128 * 1024)
                .min(self.zeroed_until * 2 + 4096)
                .min(cap - distance(self.head, self.tail))
                .min(cap - real_index(self.tail));

            let pipe_slice_mut = unsafe {
                let from = real_index(self.tail);
                let to = from + n;

                if self.zeroed_until < to {
                    self.inner
                        .buffer
                        .add(self.zeroed_until)
                        .write_bytes(0u8, to - self.zeroed_until);
                    self.zeroed_until = to;
                }

                slice::from_raw_parts_mut(self.inner.buffer.add(from), n)
            };

            let n = src.read(pipe_slice_mut)?;
            count += n;

            if n == 0 || self.inner.closed.load(Ordering::Relaxed) {
                return Poll::Ready(Ok(count));
            }

            if self.tail + n < 2 * cap {
                self.tail += n;
            } else {
                self.tail = 0;
            }

            self.inner.tail.store(self.tail, Ordering::Release);

            self.inner.reader.wake();
        }
    }
}

fn maybe_yield(cx: &mut Context<'_>) -> Poll<()> {
    if rander::usize(..100) == 0 {
        cx.waker().wake_by_ref();
        Poll::Pending
    } else {
        Poll::Ready(())
    }
}
