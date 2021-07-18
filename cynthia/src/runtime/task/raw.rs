use core::cell::UnsafeCell;
use core::future::Future;
use core::mem::{self, ManuallyDrop};
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::alloc::Layout;

use crate::runtime::task::header::Header;
use crate::runtime::task::state::*;
use crate::runtime::task::utils::{abort, abort_on_panic, extend};
use crate::runtime::task::Runnable;

pub(crate) struct TaskVTable {
    pub(crate) schedule: unsafe fn(*const ()),
    pub(crate) drop_future: unsafe fn(*const ()),
    pub(crate) get_output: unsafe fn(*const ()) -> *const (),
    pub(crate) drop_ref: unsafe fn(ptr: *const ()),
    pub(crate) destroy: unsafe fn(*const ()),
    pub(crate) run: unsafe fn(*const ()) -> bool,
    pub(crate) clone_waker: unsafe fn(ptr: *const ()) -> RawWaker,
}

#[derive(Clone, Copy)]
pub(crate) struct TaskLayout {
    pub(crate) layout: Layout,
    pub(crate) offset_s: usize,
    pub(crate) offset_f: usize,
    pub(crate) offset_r: usize,
}

pub(crate) struct RawTask<F, T, S> {
    pub(crate) header: *const Header,
    pub(crate) schedule: *const S,
    pub(crate) future: *mut F,
    pub(crate) output: *mut T,
}

impl<F, T, S> Copy for RawTask<F, T, S> {}

impl<F, T, S> Clone for RawTask<F, T, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<F, T, S> RawTask<F, T, S>
where
    F: Future<Output = T>,
    S: Fn(Runnable),
{
    const RAW_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone_waker,
        Self::wake,
        Self::wake_by_ref,
        Self::drop_waker,
    );

    pub(crate) fn allocate(future: F, schedule: S) -> NonNull<()> {
        let task_layout = abort_on_panic(|| Self::task_layout());

        unsafe {
            let ptr = match NonNull::new(std::alloc::alloc(task_layout.layout) as *mut ()) {
                None => abort(),
                Some(p) => p,
            };

            let raw = Self::from_ptr(ptr.as_ptr());

            (raw.header as *mut Header).write(Header {
                state: AtomicUsize::new(SCHEDULED | TASK | REFERENCE),
                awaiter: UnsafeCell::new(None),
                vtable: &TaskVTable {
                    schedule: Self::schedule,
                    drop_future: Self::drop_future,
                    get_output: Self::get_output,
                    drop_ref: Self::drop_ref,
                    destroy: Self::destroy,
                    run: Self::run,
                    clone_waker: Self::clone_waker,
                },
            });

            (raw.schedule as *mut S).write(schedule);

            raw.future.write(future);

            ptr
        }
    }

    pub(crate) fn from_ptr(ptr: *const ()) -> Self {
        let task_layout = Self::task_layout();
        let p = ptr as *const u8;

        unsafe {
            Self {
                header: p as *const Header,
                schedule: p.add(task_layout.offset_s) as *const S,
                future: p.add(task_layout.offset_f) as *mut F,
                output: p.add(task_layout.offset_r) as *mut T,
            }
        }
    }

    fn task_layout() -> TaskLayout {
        let layout_header = Layout::new::<Header>();
        let layout_s = Layout::new::<S>();
        let layout_f = Layout::new::<F>();
        let layout_r = Layout::new::<T>();

        let size_union = layout_f.size().max(layout_r.size());
        let align_union = layout_f.align().max(layout_r.align());
        let layout_union = unsafe { Layout::from_size_align_unchecked(size_union, align_union) };

        let layout = layout_header;
        let (layout, offset_s) = extend(layout, layout_s);
        let (layout, offset_union) = extend(layout, layout_union);
        let offset_f = offset_union;
        let offset_r = offset_union;

        TaskLayout {
            layout,
            offset_s,
            offset_f,
            offset_r,
        }
    }

    unsafe fn wake(ptr: *const ()) {
        if mem::size_of::<S>() > 0 {
            Self::wake_by_ref(ptr);
            Self::drop_waker(ptr);
            return;
        }

        let raw = Self::from_ptr(ptr);
        let mut state = (*raw.header).state.load(Ordering::Acquire);

        loop {
            if state & (COMPLETED | CLOSED) != 0 {
                Self::drop_waker(ptr);
                break;
            }

            if state & SCHEDULED != 0 {
                match (*raw.header).state.compare_exchange_weak(
                    state,
                    state,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        Self::drop_waker(ptr);
                        break;
                    }
                    Err(s) => state = s,
                }
            } else {
                match (*raw.header).state.compare_exchange_weak(
                    state,
                    state | SCHEDULED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        if state & RUNNING == 0 {
                            Self::schedule(ptr);
                        } else {
                            Self::drop_waker(ptr);
                        }

                        break;
                    }
                    Err(s) => state = s,
                }
            }
        }
    }

    unsafe fn wake_by_ref(ptr: *const ()) {
        let raw = Self::from_ptr(ptr);

        let mut state = (*raw.header).state.load(Ordering::Acquire);

        loop {
            if state & (COMPLETED | CLOSED) != 0 {
                break;
            }

            if state & SCHEDULED != 0 {
                match (*raw.header).state.compare_exchange_weak(
                    state,
                    state,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(s) => state = s,
                }
            } else {
                let new = if state & RUNNING == 0 {
                    (state | SCHEDULED) + REFERENCE
                } else {
                    state | SCHEDULED
                };

                match (*raw.header).state.compare_exchange_weak(
                    state,
                    new,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        if state & RUNNING == 0 {
                            if state > isize::max_value() as usize {
                                abort();
                            }

                            let task = Runnable {
                                nice: 0,
                                ptr: NonNull::new_unchecked(ptr as *mut ()),
                            };

                            (*raw.schedule)(task);
                        }

                        break;
                    }
                    Err(s) => state = s,
                }
            }
        }
    }

    unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
        let raw = Self::from_ptr(ptr);
        let state = (*raw.header).state.fetch_add(REFERENCE, Ordering::Relaxed);

        if state > isize::max_value() as usize {
            abort();
        }

        RawWaker::new(ptr, &Self::RAW_WAKER_VTABLE)
    }

    unsafe fn drop_waker(ptr: *const ()) {
        let raw = Self::from_ptr(ptr);

        let new = (*raw.header).state.fetch_sub(REFERENCE, Ordering::AcqRel) - REFERENCE;

        if new & !(REFERENCE - 1) == 0 && new & TASK == 0 {
            if new & (COMPLETED | CLOSED) == 0 {
                (*raw.header)
                    .state
                    .store(SCHEDULED | CLOSED | REFERENCE, Ordering::Release);
                Self::schedule(ptr);
            } else {
                Self::destroy(ptr);
            }
        }
    }

    unsafe fn drop_ref(ptr: *const ()) {
        let raw = Self::from_ptr(ptr);

        let new = (*raw.header).state.fetch_sub(REFERENCE, Ordering::AcqRel) - REFERENCE;

        if new & !(REFERENCE - 1) == 0 && new & TASK == 0 {
            Self::destroy(ptr);
        }
    }

    unsafe fn schedule(ptr: *const ()) {
        let raw = Self::from_ptr(ptr);

        let _waker;
        if mem::size_of::<S>() > 0 {
            _waker = Waker::from_raw(Self::clone_waker(ptr));
        }

        let task = Runnable {
            nice: 0,
            ptr: NonNull::new_unchecked(ptr as *mut ()),
        };
        (*raw.schedule)(task);
    }

    unsafe fn drop_future(ptr: *const ()) {
        let raw = Self::from_ptr(ptr);

        abort_on_panic(|| {
            raw.future.drop_in_place();
        })
    }

    unsafe fn get_output(ptr: *const ()) -> *const () {
        let raw = Self::from_ptr(ptr);
        raw.output as *const ()
    }

    unsafe fn destroy(ptr: *const ()) {
        let raw = Self::from_ptr(ptr);
        let task_layout = Self::task_layout();

        abort_on_panic(|| {
            (raw.schedule as *mut S).drop_in_place();
        });

        std::alloc::dealloc(ptr as *mut u8, task_layout.layout);
    }

    unsafe fn run(ptr: *const ()) -> bool {
        let raw = Self::from_ptr(ptr);

        let waker = ManuallyDrop::new(Waker::from_raw(RawWaker::new(ptr, &Self::RAW_WAKER_VTABLE)));
        let cx = &mut Context::from_waker(&waker);

        let mut state = (*raw.header).state.load(Ordering::Acquire);

        loop {
            if state & CLOSED != 0 {
                Self::drop_future(ptr);

                let state = (*raw.header).state.fetch_and(!SCHEDULED, Ordering::AcqRel);

                let mut awaiter = None;
                if state & AWAITER != 0 {
                    awaiter = (*raw.header).take(None);
                }

                Self::drop_ref(ptr);

                if let Some(w) = awaiter {
                    abort_on_panic(|| w.wake());
                }
                return false;
            }

            match (*raw.header).state.compare_exchange_weak(
                state,
                (state & !SCHEDULED) | RUNNING,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    state = (state & !SCHEDULED) | RUNNING;
                    break;
                }
                Err(s) => state = s,
            }
        }

        let guard = Guard(raw);
        let poll = <F as Future>::poll(Pin::new_unchecked(&mut *raw.future), cx);
        mem::forget(guard);

        match poll {
            Poll::Ready(out) => {
                Self::drop_future(ptr);
                raw.output.write(out);

                loop {
                    let new = if state & TASK == 0 {
                        (state & !RUNNING & !SCHEDULED) | COMPLETED | CLOSED
                    } else {
                        (state & !RUNNING & !SCHEDULED) | COMPLETED
                    };

                    match (*raw.header).state.compare_exchange_weak(
                        state,
                        new,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => {
                            if state & TASK == 0 || state & CLOSED != 0 {
                                abort_on_panic(|| raw.output.drop_in_place());
                            }

                            let mut awaiter = None;
                            if state & AWAITER != 0 {
                                awaiter = (*raw.header).take(None);
                            }

                            Self::drop_ref(ptr);

                            if let Some(w) = awaiter {
                                abort_on_panic(|| w.wake());
                            }
                            break;
                        }
                        Err(s) => state = s,
                    }
                }
            }
            Poll::Pending => {
                let mut future_dropped = false;

                loop {
                    let new = if state & CLOSED != 0 {
                        state & !RUNNING & !SCHEDULED
                    } else {
                        state & !RUNNING
                    };

                    if state & CLOSED != 0 && !future_dropped {
                        Self::drop_future(ptr);
                        future_dropped = true;
                    }

                    match (*raw.header).state.compare_exchange_weak(
                        state,
                        new,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(state) => {
                            if state & CLOSED != 0 {
                                let mut awaiter = None;
                                if state & AWAITER != 0 {
                                    awaiter = (*raw.header).take(None);
                                }

                                Self::drop_ref(ptr);

                                if let Some(w) = awaiter {
                                    abort_on_panic(|| w.wake());
                                }
                            } else if state & SCHEDULED != 0 {
                                Self::schedule(ptr);
                                return true;
                            } else {
                                Self::drop_ref(ptr);
                            }
                            break;
                        }
                        Err(s) => state = s,
                    }
                }
            }
        }

        return false;

        struct Guard<F, T, S>(RawTask<F, T, S>)
        where
            F: Future<Output = T>,
            S: Fn(Runnable);

        impl<F, T, S> Drop for Guard<F, T, S>
        where
            F: Future<Output = T>,
            S: Fn(Runnable),
        {
            fn drop(&mut self) {
                let raw = self.0;
                let ptr = raw.header as *const ();

                unsafe {
                    let mut state = (*raw.header).state.load(Ordering::Acquire);

                    loop {
                        if state & CLOSED != 0 {
                            RawTask::<F, T, S>::drop_future(ptr);

                            (*raw.header)
                                .state
                                .fetch_and(!RUNNING & !SCHEDULED, Ordering::AcqRel);

                            let mut awaiter = None;
                            if state & AWAITER != 0 {
                                awaiter = (*raw.header).take(None);
                            }

                            RawTask::<F, T, S>::drop_ref(ptr);

                            if let Some(w) = awaiter {
                                abort_on_panic(|| w.wake());
                            }
                            break;
                        }

                        match (*raw.header).state.compare_exchange_weak(
                            state,
                            (state & !RUNNING & !SCHEDULED) | CLOSED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        ) {
                            Ok(state) => {
                                RawTask::<F, T, S>::drop_future(ptr);

                                let mut awaiter = None;
                                if state & AWAITER != 0 {
                                    awaiter = (*raw.header).take(None);
                                }

                                RawTask::<F, T, S>::drop_ref(ptr);

                                if let Some(w) = awaiter {
                                    abort_on_panic(|| w.wake());
                                }
                                break;
                            }
                            Err(s) => state = s,
                        }
                    }
                }
            }
        }
    }
}
