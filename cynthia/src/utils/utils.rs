#[cfg(feature = "default")]
#[inline]
pub fn abort_on_panic<T>(f: impl FnOnce() -> T) -> T {
    struct Bomb;

    impl Drop for Bomb {
        fn drop(&mut self) {
            std::process::abort();
        }
    }

    let bomb = Bomb;
    let t = f();
    std::mem::forget(bomb);
    t
}

#[cfg(feature = "unstable")]
pub fn random(n: u32) -> u32 {
    use std::cell::Cell;
    use std::num::Wrapping;

    thread_local! {
        static RNG: Cell<Wrapping<u32>> = {
            let mut x = 0i32;
            let r = &mut x;
            let addr = r as *mut i32 as usize;
            Cell::new(Wrapping(addr as u32))
        }
    }

    RNG.with(|rng| {
        let mut x = rng.get();
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        rng.set(x);

        ((u64::from(x.0)).wrapping_mul(u64::from(n)) >> 32) as u32
    })
}

pub(crate) trait Context {
    fn context(self, message: impl Fn() -> String) -> Self;
}

#[cfg(all(
    not(target_os = "unknown"),
    any(feature = "default", feature = "unstable")
))]
mod timer {
    pub type Timer = crate::io::Timer;
}

#[cfg(any(feature = "unstable", feature = "default"))]
pub(crate) fn timer_after(dur: std::time::Duration) -> timer::Timer {
    Timer::after(dur)
}

#[cfg(any(all(target_arch = "wasm32", feature = "default"),))]
mod timer {
    use std::pin::Pin;
    use std::task::Poll;

    use gloo_timers::future::TimeoutFuture;

    #[derive(Debug)]
    pub(crate) struct Timer(TimeoutFuture);

    impl Timer {
        pub(crate) fn after(dur: std::time::Duration) -> Self {
            Timer(TimeoutFuture::new(dur.as_millis() as u32))
        }
    }

    impl std::future::Future for Timer {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
            match Pin::new(&mut self.0).poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(_) => Poll::Ready(()),
            }
        }
    }
}

#[cfg(any(feature = "unstable", feature = "default"))]
pub(crate) use timer::*;

#[cfg(feature = "default")]
#[doc(hidden)]
#[allow(unused_macros)]
macro_rules! defer {
    ($($body:tt)*) => {
        let _guard = {
            pub struct Guard<F: FnOnce()>(Option<F>);

            impl<F: FnOnce()> Drop for Guard<F> {
                fn drop(&mut self) {
                    (self.0).take().map(|f| f());
                }
            }

            Guard(Some(|| {
                let _ = { $($body)* };
            }))
        };
    };
}

#[doc(hidden)]
#[allow(unused_macros)]
macro_rules! cfg_unstable {
    ($($item:item)*) => {
        $(
            #[cfg(feature = "unstable")]
            #[cfg_attr(feature = "docs", doc(cfg(unstable)))]
            $item
        )*
    }
}

#[doc(hidden)]
#[allow(unused_macros)]
macro_rules! cfg_unstable_default {
    ($($item:item)*) => {
        $(
            #[cfg(all(feature = "default", feature = "unstable"))]
            #[cfg_attr(feature = "docs", doc(unstable))]
            $item
        )*
    }
}

#[doc(hidden)]
#[allow(unused_macros)]
macro_rules! cfg_unix {
    ($($item:item)*) => {
        $(
            #[cfg(any(unix, feature = "docs"))]
            #[cfg_attr(feature = "docs", doc(cfg(unix)))]
            $item
        )*
    }
}

#[doc(hidden)]
#[allow(unused_macros)]
macro_rules! cfg_windows {
    ($($item:item)*) => {
        $(
            #[cfg(any(windows, feature = "docs"))]
            #[cfg_attr(feature = "docs", doc(cfg(windows)))]
            $item
        )*
    }
}

#[doc(hidden)]
#[allow(unused_macros)]
macro_rules! cfg_docs {
    ($($item:item)*) => {
        $(
            #[cfg(feature = "docs")]
            $item
        )*
    }
}

#[doc(hidden)]
#[allow(unused_macros)]
macro_rules! cfg_not_docs {
    ($($item:item)*) => {
        $(
            #[cfg(not(feature = "docs"))]
            $item
        )*
    }
}

#[allow(unused_macros)]
#[doc(hidden)]
macro_rules! cfg_std {
    ($($item:item)*) => {
        $(
            #[cfg(feature = "std")]
            $item
        )*
    }
}

#[allow(unused_macros)]
#[doc(hidden)]
macro_rules! cfg_alloc {
    ($($item:item)*) => {
        $(
            #[cfg(feature = "alloc")]
            $item
        )*
    }
}

#[allow(unused_macros)]
#[doc(hidden)]
macro_rules! cfg_default {
    ($($item:item)*) => {
        $(
            #[cfg(feature = "default")]
            $item
        )*
    }
}

#[allow(unused_macros)]
#[doc(hidden)]
macro_rules! extension_trait {
    (
        #[doc = $doc:tt]
        pub trait $name:ident {
            $($body_base:tt)*
        }

        #[doc = $doc_ext:tt]
        pub trait $ext:ident: $base:path {
            $($body_ext:tt)*
        }

        $($imp:item)*
    ) => {
        #[allow(dead_code)]
        mod owned {
            #[doc(hidden)]
            pub struct ImplFuture<T>(core::marker::PhantomData<T>);
        }

        #[allow(dead_code)]
        mod borrowed {
            #[doc(hidden)]
            pub struct ImplFuture<'a, T>(core::marker::PhantomData<&'a T>);
        }

        #[cfg(feature = "docs")]
        #[doc = $doc]
        pub trait $name {
            extension_trait!(@doc () $($body_base)* $($body_ext)*);
        }

        #[cfg(not(feature = "docs"))]
        pub use $base as $name;

        #[doc = $doc_ext]
        pub trait $ext: $name {
            extension_trait!(@ext () $($body_ext)*);
        }

        impl<T: $name + ?Sized> $ext for T {}

        $(#[cfg(feature = "docs")] $imp)*
    };

    (@doc ($($head:tt)*) -> impl Future<Output = $out:ty> $(+ $lt:lifetime)? [$f:ty] $($tail:tt)*) => {
        extension_trait!(@doc ($($head)* -> owned::ImplFuture<$out>) $($tail)*);
    };
    (@ext ($($head:tt)*) -> impl Future<Output = $out:ty> $(+ $lt:lifetime)? [$f:ty] $($tail:tt)*) => {
        extension_trait!(@ext ($($head)* -> $f) $($tail)*);
    };

    (@doc ($($head:tt)*) -> impl Future<Output = $out:ty> + $lt:lifetime [$f:ty] $($tail:tt)*) => {
        extension_trait!(@doc ($($head)* -> borrowed::ImplFuture<$lt, $out>) $($tail)*);
    };
    (@ext ($($head:tt)*) -> impl Future<Output = $out:ty> + $lt:lifetime [$f:ty] $($tail:tt)*) => {
        extension_trait!(@ext ($($head)* -> $f) $($tail)*);
    };

    (@doc ($($head:tt)*) $token:tt $($tail:tt)*) => {
        extension_trait!(@doc ($($head)* $token) $($tail)*);
    };
    (@ext ($($head:tt)*) $token:tt $($tail:tt)*) => {
        extension_trait!(@ext ($($head)* $token) $($tail)*);
    };

    (@doc ($($head:tt)*)) => { $($head)* };
    (@ext ($($head:tt)*)) => { $($head)* };

    ($import:item $($tail:tt)*) => {
        #[cfg(feature = "docs")]
        $import

        extension_trait!($($tail)*);
    };
}
