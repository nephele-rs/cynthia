use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{fmt, mem, process};

use crate::platform::event::Event;
use crate::platform::lock::mutex::{Mutex, MutexGuard};

const WRITER_BIT: usize = 1;
const ONE_READER: usize = 2;

pub struct RwLock<T: ?Sized> {
    mutex: Mutex<()>,
    no_readers: Event,
    no_writer: Event,
    state: AtomicUsize,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send + ?Sized> Send for RwLock<T> {}
unsafe impl<T: Send + Sync + ?Sized> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    pub const fn new(t: T) -> RwLock<T> {
        RwLock {
            mutex: Mutex::new(()),
            no_readers: Event::new(),
            no_writer: Event::new(),
            state: AtomicUsize::new(0),
            value: UnsafeCell::new(t),
        }
    }

    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: ?Sized> RwLock<T> {
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        let mut state = self.state.load(Ordering::Acquire);

        loop {
            if state & WRITER_BIT != 0 {
                return None;
            }

            if state > std::isize::MAX as usize {
                process::abort();
            }

            match self.state.compare_exchange(
                state,
                state + ONE_READER,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(RwLockReadGuard(self)),
                Err(s) => state = s,
            }
        }
    }

    pub async fn read(&self) -> RwLockReadGuard<'_, T> {
        let mut state = self.state.load(Ordering::Acquire);

        loop {
            if state & WRITER_BIT == 0 {
                if state > std::isize::MAX as usize {
                    process::abort();
                }

                match self.state.compare_exchange(
                    state,
                    state + ONE_READER,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return RwLockReadGuard(self),
                    Err(s) => state = s,
                }
            } else {
                let listener = self.no_writer.listen();

                if self.state.load(Ordering::SeqCst) & WRITER_BIT != 0 {
                    listener.await;
                    self.no_writer.notify(1);
                }

                state = self.state.load(Ordering::Acquire);
            }
        }
    }

    pub fn try_upgradable_read(&self) -> Option<RwLockUpgradableReadGuard<'_, T>> {
        let lock = self.mutex.try_lock()?;

        let mut state = self.state.load(Ordering::Acquire);

        if state > std::isize::MAX as usize {
            process::abort();
        }

        loop {
            match self.state.compare_exchange(
                state,
                state + ONE_READER,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(RwLockUpgradableReadGuard {
                        reader: RwLockReadGuard(self),
                        reserved: lock,
                    })
                }
                Err(s) => state = s,
            }
        }
    }

    pub async fn upgradable_read(&self) -> RwLockUpgradableReadGuard<'_, T> {
        let lock = self.mutex.lock().await;

        let mut state = self.state.load(Ordering::Acquire);

        if state > std::isize::MAX as usize {
            process::abort();
        }

        loop {
            match self.state.compare_exchange(
                state,
                state + ONE_READER,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return RwLockUpgradableReadGuard {
                        reader: RwLockReadGuard(self),
                        reserved: lock,
                    }
                }
                Err(s) => state = s,
            }
        }
    }

    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        let lock = self.mutex.try_lock()?;

        if self
            .state
            .compare_exchange(0, WRITER_BIT, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Some(RwLockWriteGuard {
                writer: RwLockWriteGuardInner(self),
                reserved: lock,
            })
        } else {
            None
        }
    }

    pub async fn write(&self) -> RwLockWriteGuard<'_, T> {
        let lock = self.mutex.lock().await;

        self.state.fetch_or(WRITER_BIT, Ordering::SeqCst);
        let guard = RwLockWriteGuard {
            writer: RwLockWriteGuardInner(self),
            reserved: lock,
        };

        while self.state.load(Ordering::SeqCst) != WRITER_BIT {
            let listener = self.no_readers.listen();

            if self.state.load(Ordering::Acquire) != WRITER_BIT {
                listener.await;
            }
        }

        guard
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { &mut *self.value.get() }
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Locked;
        impl fmt::Debug for Locked {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("<locked>")
            }
        }

        match self.try_read() {
            None => f.debug_struct("RwLock").field("value", &Locked).finish(),
            Some(guard) => f.debug_struct("RwLock").field("value", &&*guard).finish(),
        }
    }
}

impl<T> From<T> for RwLock<T> {
    fn from(val: T) -> RwLock<T> {
        RwLock::new(val)
    }
}

impl<T: Default + ?Sized> Default for RwLock<T> {
    fn default() -> RwLock<T> {
        RwLock::new(Default::default())
    }
}

pub struct RwLockReadGuard<'a, T: ?Sized>(&'a RwLock<T>);

unsafe impl<T: Sync + ?Sized> Send for RwLockReadGuard<'_, T> {}
unsafe impl<T: Sync + ?Sized> Sync for RwLockReadGuard<'_, T> {}

impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        if self.0.state.fetch_sub(ONE_READER, Ordering::SeqCst) & !WRITER_BIT == ONE_READER {
            self.0.no_readers.notify(1);
        }
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for RwLockReadGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for RwLockReadGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.0.value.get() }
    }
}

pub struct RwLockUpgradableReadGuard<'a, T: ?Sized> {
    reader: RwLockReadGuard<'a, T>,
    reserved: MutexGuard<'a, ()>,
}

unsafe impl<T: Send + Sync + ?Sized> Send for RwLockUpgradableReadGuard<'_, T> {}
unsafe impl<T: Sync + ?Sized> Sync for RwLockUpgradableReadGuard<'_, T> {}

impl<'a, T: ?Sized> RwLockUpgradableReadGuard<'a, T> {
    fn into_writer(self) -> RwLockWriteGuard<'a, T> {
        let writer = RwLockWriteGuard {
            writer: RwLockWriteGuardInner(self.reader.0),
            reserved: self.reserved,
        };
        mem::forget(self.reader);
        writer
    }

    pub fn downgrade(guard: Self) -> RwLockReadGuard<'a, T> {
        guard.reader
    }

    pub fn try_upgrade(guard: Self) -> Result<RwLockWriteGuard<'a, T>, Self> {
        if guard
            .reader
            .0
            .state
            .compare_exchange(ONE_READER, WRITER_BIT, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Ok(guard.into_writer())
        } else {
            Err(guard)
        }
    }

    pub async fn upgrade(guard: Self) -> RwLockWriteGuard<'a, T> {
        guard
            .reader
            .0
            .state
            .fetch_sub(ONE_READER - WRITER_BIT, Ordering::SeqCst);

        let guard = guard.into_writer();

        while guard.writer.0.state.load(Ordering::SeqCst) != WRITER_BIT {
            let listener = guard.writer.0.no_readers.listen();

            if guard.writer.0.state.load(Ordering::Acquire) != WRITER_BIT {
                listener.await;
            }
        }

        guard
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for RwLockUpgradableReadGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for RwLockUpgradableReadGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: ?Sized> Deref for RwLockUpgradableReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.reader.0.value.get() }
    }
}

struct RwLockWriteGuardInner<'a, T: ?Sized>(&'a RwLock<T>);

impl<T: ?Sized> Drop for RwLockWriteGuardInner<'_, T> {
    fn drop(&mut self) {
        self.0.state.fetch_and(!WRITER_BIT, Ordering::SeqCst);
        self.0.no_writer.notify(1);
    }
}

pub struct RwLockWriteGuard<'a, T: ?Sized> {
    writer: RwLockWriteGuardInner<'a, T>,
    reserved: MutexGuard<'a, ()>,
}

unsafe impl<T: Send + ?Sized> Send for RwLockWriteGuard<'_, T> {}
unsafe impl<T: Sync + ?Sized> Sync for RwLockWriteGuard<'_, T> {}

impl<'a, T: ?Sized> RwLockWriteGuard<'a, T> {
    pub fn downgrade(guard: Self) -> RwLockReadGuard<'a, T> {
        guard
            .writer
            .0
            .state
            .fetch_add(ONE_READER - WRITER_BIT, Ordering::SeqCst);

        guard.writer.0.no_writer.notify(1);

        let new_guard = RwLockReadGuard(guard.writer.0);
        mem::forget(guard.writer);
        new_guard
    }

    pub fn downgrade_to_upgradable(guard: Self) -> RwLockUpgradableReadGuard<'a, T> {
        guard
            .writer
            .0
            .state
            .fetch_add(ONE_READER - WRITER_BIT, Ordering::SeqCst);

        let new_guard = RwLockUpgradableReadGuard {
            reader: RwLockReadGuard(guard.writer.0),
            reserved: guard.reserved,
        };
        mem::forget(guard.writer);
        new_guard
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for RwLockWriteGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for RwLockWriteGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.writer.0.value.get() }
    }
}

impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.writer.0.value.get() }
    }
}
