use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use std::{fmt, thread};

use crate::platform::event::Event;

pub struct Mutex<T> {
    state: AtomicUsize,
    lock_ops: Event,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub fn new(data: T) -> Mutex<T> {
        Mutex {
            state: AtomicUsize::new(0),
            lock_ops: Event::new(),
            data: UnsafeCell::new(data),
        }
    }

    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        if let Some(guard) = self.try_lock() {
            return guard;
        }
        self.lock_slow()
    }

    #[cold]
    fn lock_slow(&self) -> MutexGuard<'_, T> {
        for step in 0..10 {
            match self
                .state
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire)
            {
                Ok(0) => return MutexGuard(self),

                Err(1) => {}

                _ => break,
            }

            if step <= 3 {
                for _ in 0..1 << step {
                    std::hint::spin_loop();
                }
            } else {
                thread::yield_now();
            }
        }

        let start = Instant::now();

        loop {
            let listener = self.lock_ops.listen();

            match self
                .state
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire)
            {
                Ok(0) => return MutexGuard(self),
                Err(1) => {}
                _ => break,
            }

            listener.wait();

            match self
                .state
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire)
            {
                Ok(0) => return MutexGuard(self),

                Err(1) => {}

                _ => {
                    self.lock_ops.notify(1);
                    break;
                }
            }

            if start.elapsed() > Duration::from_micros(500) {
                break;
            }
        }

        self.state.fetch_add(2, Ordering::Release);

        let _call = CallOnDrop(|| {
            self.state.fetch_sub(2, Ordering::Release);
        });

        loop {
            let listener = self.lock_ops.listen();

            match self
                .state
                .compare_exchange(2, 2 | 1, Ordering::Acquire, Ordering::Acquire)
            {
                Ok(2) => return MutexGuard(self),

                Err(s) => {
                    if s % 2 == 1 {
                        {}
                    }
                }

                _ => {
                    self.lock_ops.notify(1);
                }
            }

            listener.wait();

            if self.state.fetch_or(1, Ordering::Acquire) % 2 == 0 {
                return MutexGuard(self);
            }
        }
    }

    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        match self
            .state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire)
        {
            Ok(0) => return Some(MutexGuard(self)),
            Err(_s) => return None,
            _ => return None,
        }
    }

    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { &mut *self.data.get() }
    }
}

impl<T: fmt::Debug> fmt::Debug for Mutex<T> {
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

impl<T: Default> Default for Mutex<T> {
    fn default() -> Mutex<T> {
        Mutex::new(Default::default())
    }
}

pub struct MutexGuard<'a, T>(&'a Mutex<T>);

unsafe impl<T: Send> Send for MutexGuard<'_, T> {}
unsafe impl<T: Sync> Sync for MutexGuard<'_, T> {}

impl<'a, T> MutexGuard<'a, T> {
    pub fn source(guard: &MutexGuard<'a, T>) -> &'a Mutex<T> {
        guard.0
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.0.state.fetch_sub(1, Ordering::Release);

        if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
            self.0.lock_ops.notify_relaxed(1);
        } else {
            self.0.lock_ops.notify(1);
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: fmt::Display> fmt::Display for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.0.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
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
