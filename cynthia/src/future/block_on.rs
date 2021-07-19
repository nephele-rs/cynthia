pub use core::future::Future;
use core::task::{Context, Poll};
use std::cell::RefCell;
use std::task::Waker;

use crate::platform::parking::Parker;
use crate::platform::waker::waker_fn;

pub use cynthia_macros::main as _;

#[cfg(feature = "std")]
pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    crate::pin!(future);

    fn parker_waker_pair() -> (Parker, Waker) {
        let parker = Parker::new();
        let unparker = parker.unparker();
        let waker = waker_fn(move || {
            unparker.unpark();
        });
        (parker, waker)
    }

    thread_local! {
        static LOCAL: RefCell<(Parker, Waker)> = RefCell::new(parker_waker_pair());
    }

    LOCAL.with(|cache| match cache.try_borrow_mut() {
        Ok(cache) => {
            let (parker, waker) = &*cache;
            let cx = &mut Context::from_waker(&waker);

            loop {
                match future.as_mut().poll(cx) {
                    Poll::Ready(output) => return output,
                    Poll::Pending => parker.park(),
                }
            }
        }
        Err(_) => {
            let (parker, waker) = parker_waker_pair();
            let cx = &mut Context::from_waker(&waker);

            loop {
                match future.as_mut().poll(cx) {
                    Poll::Ready(output) => return output,
                    Poll::Pending => parker.park(),
                }
            }
        }
    })
}
