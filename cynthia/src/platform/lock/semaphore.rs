use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::platform::event::Event;

#[derive(Debug)]
pub struct Semaphore {
    count: AtomicUsize,
    event: Event,
}

impl Semaphore {
    pub const fn new(n: usize) -> Semaphore {
        Semaphore {
            count: AtomicUsize::new(n),
            event: Event::new(),
        }
    }

    pub fn try_acquire(&self) -> Option<SemaphoreGuard<'_>> {
        let mut count = self.count.load(Ordering::Acquire);
        loop {
            if count == 0 {
                return None;
            }

            match self.count.compare_exchange_weak(
                count,
                count - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(SemaphoreGuard(self)),
                Err(c) => count = c,
            }
        }
    }

    pub async fn acquire(&self) -> SemaphoreGuard<'_> {
        let mut listener = None;

        loop {
            if let Some(guard) = self.try_acquire() {
                return guard;
            }

            match listener.take() {
                None => listener = Some(self.event.listen()),
                Some(l) => l.await,
            }
        }
    }
}

impl Semaphore {
    pub fn try_acquire_arc(self: &Arc<Self>) -> Option<SemaphoreGuardArc> {
        let mut count = self.count.load(Ordering::Acquire);
        loop {
            if count == 0 {
                return None;
            }

            match self.count.compare_exchange_weak(
                count,
                count - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(SemaphoreGuardArc(self.clone())),
                Err(c) => count = c,
            }
        }
    }

    pub async fn acquire_arc(self: &Arc<Self>) -> SemaphoreGuardArc {
        let mut listener = None;

        loop {
            if let Some(guard) = self.try_acquire_arc() {
                return guard;
            }

            match listener.take() {
                None => listener = Some(self.event.listen()),
                Some(l) => l.await,
            }
        }
    }
}

#[derive(Debug)]
pub struct SemaphoreGuard<'a>(&'a Semaphore);

impl Drop for SemaphoreGuard<'_> {
    fn drop(&mut self) {
        self.0.count.fetch_add(1, Ordering::AcqRel);
        self.0.event.notify(1);
    }
}

#[derive(Debug)]
pub struct SemaphoreGuardArc(Arc<Semaphore>);

impl Drop for SemaphoreGuardArc {
    fn drop(&mut self) {
        self.0.count.fetch_add(1, Ordering::AcqRel);
        self.0.event.notify(1);
    }
}
