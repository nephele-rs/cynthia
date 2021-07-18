use core::cell::UnsafeCell;
use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::Waker;

use crate::runtime::task::raw::TaskVTable;
use crate::runtime::task::state::*;
use crate::runtime::task::utils::abort_on_panic;

pub(crate) struct Header {
    pub(crate) state: AtomicUsize,
    pub(crate) awaiter: UnsafeCell<Option<Waker>>,
    pub(crate) vtable: &'static TaskVTable,
}

impl Header {
    #[inline]
    pub(crate) fn notify(&self, current: Option<&Waker>) {
        if let Some(w) = self.take(current) {
            abort_on_panic(|| w.wake());
        }
    }

    #[inline]
    pub(crate) fn take(&self, current: Option<&Waker>) -> Option<Waker> {
        let state = self.state.fetch_or(NOTIFYING, Ordering::AcqRel);

        if state & (NOTIFYING | REGISTERING) == 0 {
            let waker = unsafe { (*self.awaiter.get()).take() };

            self.state
                .fetch_and(!NOTIFYING & !AWAITER, Ordering::Release);

            if let Some(w) = waker {
                match current {
                    None => return Some(w),
                    Some(c) if !w.will_wake(c) => return Some(w),
                    Some(_) => abort_on_panic(|| drop(w)),
                }
            }
        }

        None
    }

    #[inline]
    pub(crate) fn register(&self, waker: &Waker) {
        let mut state = self.state.fetch_or(0, Ordering::Acquire);

        loop {
            debug_assert!(state & REGISTERING == 0);

            if state & NOTIFYING != 0 {
                abort_on_panic(|| waker.wake_by_ref());
                return;
            }

            match self.state.compare_exchange_weak(
                state,
                state | REGISTERING,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    state |= REGISTERING;
                    break;
                }
                Err(s) => state = s,
            }
        }

        unsafe {
            abort_on_panic(|| (*self.awaiter.get()) = Some(waker.clone()));
        }

        let mut waker = None;

        loop {
            if state & NOTIFYING != 0 {
                if let Some(w) = unsafe { (*self.awaiter.get()).take() } {
                    abort_on_panic(|| waker = Some(w));
                }
            }

            let new = if waker.is_none() {
                (state & !NOTIFYING & !REGISTERING) | AWAITER
            } else {
                state & !NOTIFYING & !REGISTERING & !AWAITER
            };

            match self
                .state
                .compare_exchange_weak(state, new, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(s) => state = s,
            }
        }

        if let Some(w) = waker {
            abort_on_panic(|| w.wake());
        }
    }
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.load(Ordering::SeqCst);

        f.debug_struct("Header")
            .field("scheduled", &(state & SCHEDULED != 0))
            .field("running", &(state & RUNNING != 0))
            .field("completed", &(state & COMPLETED != 0))
            .field("closed", &(state & CLOSED != 0))
            .field("awaiter", &(state & AWAITER != 0))
            .field("task", &(state & TASK != 0))
            .field("ref_count", &(state / REFERENCE))
            .finish()
    }
}
