use std::cell::UnsafeCell;
use std::fmt;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release};
use std::task::Waker;

pub struct AtomicWaker {
    state: AtomicUsize,
    waker: UnsafeCell<Option<Waker>>,
}

const WAITING: usize = 0;
const REGISTERING: usize = 0b01;
const WAKING: usize = 0b10;

impl AtomicWaker {
    pub const fn new() -> Self {
        trait AssertSync: Sync {}
        impl AssertSync for Waker {}

        AtomicWaker {
            state: AtomicUsize::new(WAITING),
            waker: UnsafeCell::new(None),
        }
    }

    pub fn register(&self, waker: &Waker) {
        match self
            .state
            .compare_exchange(WAITING, REGISTERING, Acquire, Acquire)
            .unwrap_or_else(|x| x)
        {
            WAITING => unsafe {
                *self.waker.get() = Some(waker.clone());

                let res = self
                    .state
                    .compare_exchange(REGISTERING, WAITING, AcqRel, Acquire);

                match res {
                    Ok(_) => {}
                    Err(actual) => {
                        debug_assert_eq!(actual, REGISTERING | WAKING);

                        let waker = (*self.waker.get()).take().unwrap();
                        self.state.swap(WAITING, AcqRel);
                        waker.wake();
                    }
                }
            },
            WAKING => {
                waker.wake_by_ref();
            }
            state => {
                debug_assert!(state == REGISTERING || state == REGISTERING | WAKING);
            }
        }
    }

    pub fn wake(&self) {
        if let Some(waker) = self.take() {
            waker.wake();
        }
    }

    pub fn take(&self) -> Option<Waker> {
        match self.state.fetch_or(WAKING, AcqRel) {
            WAITING => {
                let waker = unsafe { (*self.waker.get()).take() };

                self.state.fetch_and(!WAKING, Release);

                waker
            }
            state => {
                debug_assert!(
                    state == REGISTERING || state == REGISTERING | WAKING || state == WAKING
                );
                None
            }
        }
    }
}

impl Default for AtomicWaker {
    fn default() -> Self {
        AtomicWaker::new()
    }
}

impl fmt::Debug for AtomicWaker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AtomicWaker")
    }
}

unsafe impl Send for AtomicWaker {}
unsafe impl Sync for AtomicWaker {}
