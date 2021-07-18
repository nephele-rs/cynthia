use core::future::Future;
use core::marker::PhantomData;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use core::task::Waker;
use core::{fmt, mem};

use crate::runtime::task::header::Header;
use crate::runtime::task::raw::RawTask;
use crate::runtime::task::state::*;
use crate::runtime::task::task::Task;

pub fn spawn<F, S>(future: F, schedule: S) -> (Runnable, Task<F::Output>)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
    S: Fn(Runnable) + Send + Sync + 'static,
{
    unsafe { spawn_unchecked(future, schedule) }
}

#[cfg(feature = "std")]
pub fn spawn_local<F, S>(future: F, schedule: S) -> (Runnable, Task<F::Output>)
where
    F: Future + 'static,
    F::Output: 'static,
    S: Fn(Runnable) + Send + Sync + 'static,
{
    use std::mem::ManuallyDrop;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::thread::{self, ThreadId};

    #[inline]
    fn thread_id() -> ThreadId {
        thread_local! {
            static ID: ThreadId = thread::current().id();
        }
        ID.try_with(|id| *id)
            .unwrap_or_else(|_| thread::current().id())
    }

    struct Checked<F> {
        id: ThreadId,
        inner: ManuallyDrop<F>,
    }

    impl<F> Drop for Checked<F> {
        fn drop(&mut self) {
            assert!(
                self.id == thread_id(),
                "local task dropped by a thread that didn't spawn it"
            );
            unsafe {
                ManuallyDrop::drop(&mut self.inner);
            }
        }
    }

    impl<F: Future> Future for Checked<F> {
        type Output = F::Output;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            assert!(
                self.id == thread_id(),
                "local task polled by a thread that didn't spawn it"
            );
            unsafe { self.map_unchecked_mut(|c| &mut *c.inner).poll(cx) }
        }
    }

    let future = Checked {
        id: thread_id(),
        inner: ManuallyDrop::new(future),
    };

    unsafe { spawn_unchecked(future, schedule) }
}

pub unsafe fn spawn_unchecked<F, S>(future: F, schedule: S) -> (Runnable, Task<F::Output>)
where
    F: Future,
    S: Fn(Runnable),
{
    let ptr = if mem::size_of::<F>() >= 2048 {
        let future = std::boxed::Box::pin(future);
        RawTask::<_, F::Output, S>::allocate(future, schedule)
    } else {
        RawTask::<F, F::Output, S>::allocate(future, schedule)
    };

    let runnable = Runnable { nice: 0, ptr };

    let task = Task {
        ptr,
        _marker: PhantomData,
    };

    (runnable, task)
}

pub struct Runnable {
    pub(crate) nice: i32,
    pub(crate) ptr: NonNull<()>,
}

unsafe impl Send for Runnable {}
unsafe impl Sync for Runnable {}

#[cfg(feature = "std")]
impl std::panic::UnwindSafe for Runnable {}
#[cfg(feature = "std")]
impl std::panic::RefUnwindSafe for Runnable {}

impl Runnable {
    pub fn nice(self) -> i32 {
        self.nice
    }

    pub fn schedule(self) {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;
        mem::forget(self);

        unsafe {
            ((*header).vtable.schedule)(ptr);
        }
    }

    pub fn run(self) -> bool {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;
        mem::forget(self);

        unsafe { ((*header).vtable.run)(ptr) }
    }

    pub fn waker(&self) -> Waker {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            let raw_waker = ((*header).vtable.clone_waker)(ptr);
            Waker::from_raw(raw_waker)
        }
    }
}

impl Drop for Runnable {
    fn drop(&mut self) {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;

        unsafe {
            let mut state = (*header).state.load(Ordering::Acquire);

            loop {
                if state & (COMPLETED | CLOSED) != 0 {
                    break;
                }

                match (*header).state.compare_exchange_weak(
                    state,
                    state | CLOSED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(s) => state = s,
                }
            }

            ((*header).vtable.drop_future)(ptr);

            let state = (*header).state.fetch_and(!SCHEDULED, Ordering::AcqRel);

            if state & AWAITER != 0 {
                (*header).notify(None);
            }

            ((*header).vtable.drop_ref)(ptr);
        }
    }
}

impl fmt::Debug for Runnable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ptr = self.ptr.as_ptr();
        let header = ptr as *const Header;

        f.debug_struct("Runnable")
            .field("header", unsafe { &(*header) })
            .finish()
    }
}
