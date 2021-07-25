#![cfg_attr(feature = "std", doc = "```no_run")]
#![cfg_attr(not(feature = "std"), doc = "```ignore")]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
#[doc(hidden)]
pub use crate::future::swap::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWrite,
    AsyncWriteExt,
};
#[doc(hidden)]
pub use crate::future::{
    future::{Future, FutureExt},
    stream::{Stream, StreamExt},
};

pub mod future;
pub mod prelude;
pub mod stream;

#[cfg(feature = "std")]
pub mod swap;

#[cfg(any(feature = "unstable", feature = "default"))]
pub use timeout::{timeout, TimeoutError};
#[cfg(any(feature = "unstable", feature = "default"))]
mod timeout;

mod block_on;
pub use block_on::block_on;
