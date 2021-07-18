use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::usize;
use std::{fmt, process};

use crate::platform::event::Event;

pub struct Mutex<T: ?Sized> {
    state: AtomicUsize,
    lock_ops: Event,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send + ?Sized> Send for Mutex<T> {}
unsafe impl<T: Send + ?Sized> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(data: T) -> Mutex<T> {
        Mutex {
            state: AtomicUsize::new(0),
            lock_ops: Event::new(),
            data: UnsafeCell::new(data),
        }
    }

    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    #[inline]
    pub async fn lock(&self) -> MutexGuard<'_, T> {
        if let Some(guard) = self.try_lock() {
            return guard;
        }
        self.acquire_slow().await;
        MutexGuard(self)
    }

    #[cold]
    async fn acquire_slow(&self) {
        let start = Instant::now();

        loop {
            let listener = self.lock_ops.listen();

            match self
                .state
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire)
                .unwrap_or_else(|x| x)
            {
                0 => return,

                1 => {}

                _ => break,
            }

            listener.await;

            match self
                .state
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire)
                .unwrap_or_else(|x| x)
            {
                0 => return,

                1 => {}

                _ => {
                    self.lock_ops.notify(1);
                    break;
                }
            }

            if start.elapsed() > Duration::from_micros(500) {
                break;
            }
        }

        if self.state.fetch_add(2, Ordering::Release) > usize::MAX / 2 {
            process::abort();
        }

        let _call = CallOnDrop(|| {
            self.state.fetch_sub(2, Ordering::Release);
        });

        loop {
            let listener = self.lock_ops.listen();

            match self
                .state
                .compare_exchange(2, 2 | 1, Ordering::Acquire, Ordering::Acquire)
                .unwrap_or_else(|x| x)
            {
                2 => return,

                s if s % 2 == 1 => {}

                _ => {
                    self.lock_ops.notify(1);
                }
            }

            listener.await;

            if self.state.fetch_or(1, Ordering::Acquire) % 2 == 0 {
                return;
            }
        }
    }

    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if self
            .state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire)
            .is_ok()
        {
            Some(MutexGuard(self))
        } else {
            None
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { &mut *self.data.get() }
    }
}

impl<T: ?Sized> Mutex<T> {
    #[inline]
    pub async fn lock_arc(self: &Arc<Self>) -> MutexGuardArc<T> {
        if let Some(guard) = self.try_lock_arc() {
            return guard;
        }
        self.acquire_slow().await;
        MutexGuardArc(self.clone())
    }

    #[inline]
    pub fn try_lock_arc(self: &Arc<Self>) -> Option<MutexGuardArc<T>> {
        if self
            .state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire)
            .is_ok()
        {
            Some(MutexGuardArc(self.clone()))
        } else {
            None
        }
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Locked;
        impl fmt::Debug for Locked {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("<locked>")
            }
        }

        match self.try_lock() {
            None => f.debug_struct("Mutex").field("data", &Locked).finish(),
            Some(guard) => f.debug_struct("Mutex").field("data", &&*guard).finish(),
        }
    }
}

impl<T> From<T> for Mutex<T> {
    fn from(val: T) -> Mutex<T> {
        Mutex::new(val)
    }
}

impl<T: Default + ?Sized> Default for Mutex<T> {
    fn default() -> Mutex<T> {
        Mutex::new(Default::default())
    }
}

pub struct MutexGuard<'a, T: ?Sized>(&'a Mutex<T>);

unsafe impl<T: Send + ?Sized> Send for MutexGuard<'_, T> {}
unsafe impl<T: Sync + ?Sized> Sync for MutexGuard<'_, T> {}

impl<'a, T: ?Sized> MutexGuard<'a, T> {
    pub fn source(guard: &MutexGuard<'a, T>) -> &'a Mutex<T> {
        guard.0
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.0.state.fetch_sub(1, Ordering::Release);
        self.0.lock_ops.notify(1);
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.0.data.get() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.0.data.get() }
    }
}

pub struct MutexGuardArc<T: ?Sized>(Arc<Mutex<T>>);

unsafe impl<T: Send + ?Sized> Send for MutexGuardArc<T> {}
unsafe impl<T: Sync + ?Sized> Sync for MutexGuardArc<T> {}

impl<T: ?Sized> MutexGuardArc<T> {
    pub fn source(guard: &MutexGuardArc<T>) -> &Arc<Mutex<T>> {
        &guard.0
    }
}

impl<T: ?Sized> Drop for MutexGuardArc<T> {
    fn drop(&mut self) {
        self.0.state.fetch_sub(1, Ordering::Release);
        self.0.lock_ops.notify(1);
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for MutexGuardArc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for MutexGuardArc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: ?Sized> Deref for MutexGuardArc<T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.0.data.get() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuardArc<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.0.data.get() }
    }
}

struct CallOnDrop<F: Fn()>(F);

impl<F: Fn()> Drop for CallOnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}
