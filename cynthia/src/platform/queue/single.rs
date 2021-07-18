use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::platform::queue::{PopError, PushError};

const LOCKED: usize = 1 << 0;
const PUSHED: usize = 1 << 1;
const CLOSED: usize = 1 << 2;

pub struct Single<T> {
    state: AtomicUsize,
    slot: UnsafeCell<MaybeUninit<T>>,
}

impl<T> Single<T> {
    pub fn new() -> Single<T> {
        Single {
            state: AtomicUsize::new(0),
            slot: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    pub fn push(&self, value: T) -> Result<(), PushError<T>> {
        let state = self
            .state
            .compare_exchange(0, LOCKED | PUSHED, Ordering::SeqCst, Ordering::SeqCst)
            .unwrap_or_else(|x| x);

        if state == 0 {
            unsafe { self.slot.get().write(MaybeUninit::new(value)) }
            self.state.fetch_and(!LOCKED, Ordering::Release);
            Ok(())
        } else if state & CLOSED != 0 {
            Err(PushError::Closed(value))
        } else {
            Err(PushError::Full(value))
        }
    }

    pub fn pop(&self) -> Result<T, PopError> {
        let mut state = PUSHED;
        loop {
            let prev = self
                .state
                .compare_exchange(
                    state,
                    (state | LOCKED) & !PUSHED,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .unwrap_or_else(|x| x);

            if prev == state {
                let value = unsafe { self.slot.get().read().assume_init() };
                self.state.fetch_and(!LOCKED, Ordering::Release);
                return Ok(value);
            }

            if prev & PUSHED == 0 {
                if prev & CLOSED == 0 {
                    return Err(PopError::Empty);
                } else {
                    return Err(PopError::Closed);
                }
            }

            if prev & LOCKED == 0 {
                state = prev;
            } else {
                thread::yield_now();
                state = prev & !LOCKED;
            }
        }
    }

    pub fn len(&self) -> usize {
        if self.state.load(Ordering::SeqCst) & PUSHED == 0 {
            0
        } else {
            1
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() == 1
    }

    pub fn close(&self) -> bool {
        let state = self.state.fetch_or(CLOSED, Ordering::SeqCst);
        state & CLOSED == 0
    }

    pub fn is_closed(&self) -> bool {
        self.state.load(Ordering::SeqCst) & CLOSED != 0
    }
}

impl<T> Drop for Single<T> {
    fn drop(&mut self) {
        if *self.state.get_mut() & PUSHED != 0 {
            let value = unsafe { self.slot.get().read().assume_init() };
            drop(value);
        }
    }
}
