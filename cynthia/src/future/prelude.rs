pub use crate::future::{
    future::{Future, FutureExt as _},
    stream::{Stream, StreamExt as _},
};

#[cfg(feature = "std")]
pub use crate::future::{
    swap::{AsyncBufRead, AsyncBufReadExt as _},
    swap::{AsyncRead, AsyncReadExt as _},
    swap::{AsyncSeek, AsyncSeekExt as _},
    swap::{AsyncWrite, AsyncWriteExt as _},
};
