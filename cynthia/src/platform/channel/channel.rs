#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::{error, fmt, process, usize};
use futures_core::stream::Stream;

use crate::platform::event::{Event, EventListener};
use crate::platform::queue::{ConcurrentQueue, PopError, PushError};

struct Channel<T> {
    queue: ConcurrentQueue<T>,
    send_ops: Event,
    recv_ops: Event,
    stream_ops: Event,
    sender_count: AtomicUsize,
    receiver_count: AtomicUsize,
}

impl<T> Channel<T> {
    fn close(&self) -> bool {
        if self.queue.close() {
            self.send_ops.notify(usize::MAX);

            self.recv_ops.notify(usize::MAX);
            self.stream_ops.notify(usize::MAX);

            true
        } else {
            false
        }
    }
}

pub fn bounded<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    assert!(cap > 0, "capacity cannot be zero");

    let channel = Arc::new(Channel {
        queue: ConcurrentQueue::bounded(cap),
        send_ops: Event::new(),
        recv_ops: Event::new(),
        stream_ops: Event::new(),
        sender_count: AtomicUsize::new(1),
        receiver_count: AtomicUsize::new(1),
    });

    let s = Sender {
        channel: channel.clone(),
    };
    let r = Receiver {
        channel,
        listener: None,
    };
    (s, r)
}

pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let channel = Arc::new(Channel {
        queue: ConcurrentQueue::unbounded(),
        send_ops: Event::new(),
        recv_ops: Event::new(),
        stream_ops: Event::new(),
        sender_count: AtomicUsize::new(1),
        receiver_count: AtomicUsize::new(1),
    });

    let s = Sender {
        channel: channel.clone(),
    };
    let r = Receiver {
        channel,
        listener: None,
    };
    (s, r)
}

pub struct Sender<T> {
    channel: Arc<Channel<T>>,
}

impl<T> Sender<T> {
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        match self.channel.queue.push(msg) {
            Ok(()) => {
                self.channel.recv_ops.notify(1);

                self.channel.stream_ops.notify(usize::MAX);

                Ok(())
            }
            Err(PushError::Full(msg)) => Err(TrySendError::Full(msg)),
            Err(PushError::Closed(msg)) => Err(TrySendError::Closed(msg)),
        }
    }

    pub async fn send(&self, msg: T) -> Result<(), SendError<T>> {
        let mut listener = None;
        let mut msg = msg;

        loop {
            match self.try_send(msg) {
                Ok(()) => {
                    match self.channel.queue.capacity() {
                        Some(1) => {}
                        Some(_) | None => self.channel.send_ops.notify(1),
                    }
                    return Ok(());
                }
                Err(TrySendError::Closed(msg)) => return Err(SendError(msg)),
                Err(TrySendError::Full(m)) => msg = m,
            }

            match listener.take() {
                None => {
                    listener = Some(self.channel.send_ops.listen());
                }
                Some(l) => {
                    l.await;
                }
            }
        }
    }

    pub fn close(&self) -> bool {
        self.channel.close()
    }

    pub fn is_closed(&self) -> bool {
        self.channel.queue.is_closed()
    }

    pub fn is_empty(&self) -> bool {
        self.channel.queue.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.channel.queue.is_full()
    }

    pub fn len(&self) -> usize {
        self.channel.queue.len()
    }

    pub fn capacity(&self) -> Option<usize> {
        self.channel.queue.capacity()
    }

    pub fn receiver_count(&self) -> usize {
        self.channel.receiver_count.load(Ordering::SeqCst)
    }

    pub fn sender_count(&self) -> usize {
        self.channel.sender_count.load(Ordering::SeqCst)
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.channel.sender_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.channel.close();
        }
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sender {{ .. }}")
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Sender<T> {
        let count = self.channel.sender_count.fetch_add(1, Ordering::Relaxed);

        if count > usize::MAX / 2 {
            process::abort();
        }

        Sender {
            channel: self.channel.clone(),
        }
    }
}

pub struct Receiver<T> {
    channel: Arc<Channel<T>>,
    listener: Option<EventListener>,
}

impl<T> Receiver<T> {
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.channel.queue.pop() {
            Ok(msg) => {
                self.channel.send_ops.notify(1);

                Ok(msg)
            }
            Err(PopError::Empty) => Err(TryRecvError::Empty),
            Err(PopError::Closed) => Err(TryRecvError::Closed),
        }
    }

    pub async fn recv(&self) -> Result<T, RecvError> {
        let mut listener = None;

        loop {
            match self.try_recv() {
                Ok(msg) => {
                    match self.channel.queue.capacity() {
                        Some(1) => {}
                        Some(_) | None => self.channel.recv_ops.notify(1),
                    }
                    return Ok(msg);
                }
                Err(TryRecvError::Closed) => return Err(RecvError),
                Err(TryRecvError::Empty) => {}
            }

            match listener.take() {
                None => {
                    listener = Some(self.channel.recv_ops.listen());
                }
                Some(l) => {
                    l.await;
                }
            }
        }
    }

    pub fn close(&self) -> bool {
        self.channel.close()
    }

    pub fn is_closed(&self) -> bool {
        self.channel.queue.is_closed()
    }

    pub fn is_empty(&self) -> bool {
        self.channel.queue.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.channel.queue.is_full()
    }

    pub fn len(&self) -> usize {
        self.channel.queue.len()
    }

    pub fn capacity(&self) -> Option<usize> {
        self.channel.queue.capacity()
    }

    pub fn receiver_count(&self) -> usize {
        self.channel.receiver_count.load(Ordering::SeqCst)
    }

    pub fn sender_count(&self) -> usize {
        self.channel.sender_count.load(Ordering::SeqCst)
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        if self.channel.receiver_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.channel.close();
        }
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Receiver {{ .. }}")
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Receiver<T> {
        let count = self.channel.receiver_count.fetch_add(1, Ordering::Relaxed);

        if count > usize::MAX / 2 {
            process::abort();
        }

        Receiver {
            channel: self.channel.clone(),
            listener: None,
        }
    }
}

impl<T> Stream for Receiver<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(listener) = self.listener.as_mut() {
                futures_core::ready!(Pin::new(listener).poll(cx));
                self.listener = None;
            }

            loop {
                match self.try_recv() {
                    Ok(msg) => {
                        self.listener = None;
                        return Poll::Ready(Some(msg));
                    }
                    Err(TryRecvError::Closed) => {
                        self.listener = None;
                        return Poll::Ready(None);
                    }
                    Err(TryRecvError::Empty) => {}
                }

                match self.listener.as_mut() {
                    None => {
                        self.listener = Some(self.channel.stream_ops.listen());
                    }
                    Some(_) => {
                        break;
                    }
                }
            }
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub struct SendError<T>(pub T);

impl<T> SendError<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> error::Error for SendError<T> {}

impl<T> fmt::Debug for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SendError(..)")
    }
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sending into a closed channel")
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum TrySendError<T> {
    Full(T),
    Closed(T),
}

impl<T> TrySendError<T> {
    pub fn into_inner(self) -> T {
        match self {
            TrySendError::Full(t) => t,
            TrySendError::Closed(t) => t,
        }
    }

    pub fn is_full(&self) -> bool {
        match self {
            TrySendError::Full(_) => true,
            TrySendError::Closed(_) => false,
        }
    }

    pub fn is_closed(&self) -> bool {
        match self {
            TrySendError::Full(_) => false,
            TrySendError::Closed(_) => true,
        }
    }
}

impl<T> error::Error for TrySendError<T> {}

impl<T> fmt::Debug for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            TrySendError::Full(..) => write!(f, "Full(..)"),
            TrySendError::Closed(..) => write!(f, "Closed(..)"),
        }
    }
}

impl<T> fmt::Display for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            TrySendError::Full(..) => write!(f, "sending into a full channel"),
            TrySendError::Closed(..) => write!(f, "sending into a closed channel"),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct RecvError;

impl error::Error for RecvError {}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "receiving from an empty and closed channel")
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum TryRecvError {
    Empty,

    Closed,
}

impl TryRecvError {
    pub fn is_empty(&self) -> bool {
        match self {
            TryRecvError::Empty => true,
            TryRecvError::Closed => false,
        }
    }

    pub fn is_closed(&self) -> bool {
        match self {
            TryRecvError::Empty => false,
            TryRecvError::Closed => true,
        }
    }
}

impl error::Error for TryRecvError {}

impl fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            TryRecvError::Empty => write!(f, "receiving from an empty channel"),
            TryRecvError::Closed => write!(f, "receiving from an empty and closed channel"),
        }
    }
}
