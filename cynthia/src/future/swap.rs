use futures_core::stream::Stream;
pub use futures_io::{AsyncBufRead, AsyncRead, AsyncSeek, AsyncWrite};
use pin_project_lite::pin_project;
use std::future::Future;
pub use std::io::{Error, ErrorKind, Result, SeekFrom};
use std::io::{IoSlice, IoSliceMut};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::{cmp, fmt, mem};

use crate::ready;

const DEFAULT_BUFSZ: usize = 8 * 1024;

pub fn repeat(byte: u8) -> Repeat {
    Repeat { byte }
}

#[derive(Debug)]
pub struct Repeat {
    byte: u8,
}

impl AsyncRead for Repeat {
    #[inline]
    fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<usize>> {
        for b in &mut *buf {
            *b = self.byte;
        }
        Poll::Ready(Ok(buf.len()))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Cursor<T> {
    inner: std::io::Cursor<T>,
}

impl<T> Cursor<T> {
    pub fn new(inner: T) -> Cursor<T> {
        Cursor {
            inner: std::io::Cursor::new(inner),
        }
    }

    pub fn get_ref(&self) -> &T {
        self.inner.get_ref()
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }

    pub fn position(&self) -> u64 {
        self.inner.position()
    }

    pub fn set_position(&mut self, pos: u64) {
        self.inner.set_position(pos)
    }
}

impl<T> AsyncSeek for Cursor<T>
where
    T: AsRef<[u8]> + Unpin,
{
    fn poll_seek(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        pos: SeekFrom,
    ) -> Poll<Result<u64>> {
        Poll::Ready(std::io::Seek::seek(&mut self.inner, pos))
    }
}

impl<T> AsyncRead for Cursor<T>
where
    T: AsRef<[u8]> + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Poll::Ready(std::io::Read::read(&mut self.inner, buf))
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        Poll::Ready(std::io::Read::read_vectored(&mut self.inner, bufs))
    }
}

impl<T> AsyncBufRead for Cursor<T>
where
    T: AsRef<[u8]> + Unpin,
{
    fn poll_fill_buf(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<&[u8]>> {
        Poll::Ready(std::io::BufRead::fill_buf(&mut self.get_mut().inner))
    }

    fn consume(mut self: Pin<&mut Self>, length: usize) {
        std::io::BufRead::consume(&mut self.inner, length)
    }
}

impl AsyncWrite for Cursor<&mut [u8]> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Poll::Ready(std::io::Write::write(&mut self.inner, buf))
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize>> {
        Poll::Ready(std::io::Write::write_vectored(&mut self.inner, bufs))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(std::io::Write::flush(&mut self.inner))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.poll_flush(cx)
    }
}

impl AsyncWrite for Cursor<&mut Vec<u8>> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Poll::Ready(std::io::Write::write(&mut self.inner, buf))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.poll_flush(cx)
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(std::io::Write::flush(&mut self.inner))
    }
}

impl AsyncWrite for Cursor<Vec<u8>> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Poll::Ready(std::io::Write::write(&mut self.inner, buf))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.poll_flush(cx)
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(std::io::Write::flush(&mut self.inner))
    }
}

pin_project! {
    pub struct BufferReader<R> {
        #[pin]
        inner: R,
        buf: Box<[u8]>,
        headroom: usize,
        pos: usize,
        cap: usize,
    }
}

impl<R: AsyncRead> BufferReader<R> {
    pub fn new(inner: R) -> BufferReader<R> {
        BufferReader::with_capacity(DEFAULT_BUFSZ, inner)
    }

    pub fn with_capacity(capacity: usize, inner: R) -> BufferReader<R> {
        BufferReader {
            inner,
            buf: vec![0; capacity].into_boxed_slice(),
            headroom: 0,
            pos: 0,
            cap: 0,
        }
    }
}

impl<R> BufferReader<R> {
    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut R> {
        self.project().inner
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buf[self.pos..self.cap]
    }

    pub fn into_inner(self) -> R {
        self.inner
    }

    #[inline]
    fn discard_buffer(self: Pin<&mut Self>) {
        let this = self.project();
        *this.pos = 0;
        *this.cap = 0;
    }
}

impl<R: AsyncRead> AsyncRead for BufferReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        if self.pos == self.cap && buf.len() >= self.buf.len() {
            let res = ready!(self.as_mut().get_pin_mut().poll_read(cx, buf));
            self.discard_buffer();
            return Poll::Ready(res);
        }
        let mut rem = ready!(self.as_mut().poll_fill_buf(cx))?;
        let nread = std::io::Read::read(&mut rem, buf)?;
        self.consume(nread);
        Poll::Ready(Ok(nread))
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        let total_len = bufs.iter().map(|b| b.len()).sum::<usize>();
        if self.pos == self.cap && total_len >= self.buf.len() {
            let res = ready!(self.as_mut().get_pin_mut().poll_read_vectored(cx, bufs));
            self.discard_buffer();
            return Poll::Ready(res);
        }
        let mut rem = ready!(self.as_mut().poll_fill_buf(cx))?;
        let nread = std::io::Read::read_vectored(&mut rem, bufs)?;
        self.consume(nread);
        Poll::Ready(Ok(nread))
    }
}

impl<R: AsyncRead> AsyncBufRead for BufferReader<R> {
    fn poll_fill_buf<'a>(self: Pin<&'a mut Self>, cx: &mut Context<'_>) -> Poll<Result<&'a [u8]>> {
        let mut this = self.project();

        if *this.pos >= *this.cap {
            debug_assert!(*this.pos == *this.cap);
            *this.cap = ready!(this.inner.as_mut().poll_read(cx, this.buf))?;
            *this.pos = 0;
        }
        Poll::Ready(Ok(&this.buf[*this.pos..*this.cap]))
    }

    fn consume(self: Pin<&mut Self>, length: usize) {
        let this = self.project();
        *this.pos = cmp::min(*this.pos + length, *this.cap);
    }
}

impl<R: AsyncRead + fmt::Debug> fmt::Debug for BufferReader<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufferReader")
            .field("reader", &self.inner)
            .field(
                "buffer",
                &format_args!("{}/{}", self.cap - self.pos, self.buf.len()),
            )
            .finish()
    }
}

impl<R: AsyncSeek> AsyncSeek for BufferReader<R> {
    fn poll_seek(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        pos: SeekFrom,
    ) -> Poll<Result<u64>> {
        let result: u64;
        if let SeekFrom::Current(n) = pos {
            let remainder = (self.cap - self.pos) as i64;
            if let Some(offset) = n.checked_sub(remainder) {
                result = ready!(self
                    .as_mut()
                    .get_pin_mut()
                    .poll_seek(cx, SeekFrom::Current(offset)))?;
            } else {
                ready!(self
                    .as_mut()
                    .get_pin_mut()
                    .poll_seek(cx, SeekFrom::Current(-remainder)))?;
                self.as_mut().discard_buffer();
                result = ready!(self
                    .as_mut()
                    .get_pin_mut()
                    .poll_seek(cx, SeekFrom::Current(n)))?;
            }
        } else {
            result = ready!(self.as_mut().get_pin_mut().poll_seek(cx, pos))?;
        }
        self.discard_buffer();
        Poll::Ready(Ok(result))
    }
}

pin_project! {
    pub struct BufferWriter<W> {
        #[pin]
        inner: W,
        buf: Vec<u8>,
        written: usize,
    }
}

impl<W: AsyncWrite> BufferWriter<W> {
    pub fn new(inner: W) -> BufferWriter<W> {
        BufferWriter::with_capacity(DEFAULT_BUFSZ, inner)
    }

    pub fn with_capacity(capacity: usize, inner: W) -> BufferWriter<W> {
        BufferWriter {
            inner,
            buf: Vec::with_capacity(capacity),
            written: 0,
        }
    }

    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut W> {
        self.project().inner
    }

    pub fn into_inner(self) -> W {
        self.inner
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buf
    }

    fn poll_flush_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let mut this = self.project();
        let len = this.buf.len();
        let mut ret = Ok(());

        while *this.written < len {
            match this
                .inner
                .as_mut()
                .poll_write(cx, &this.buf[*this.written..])
            {
                Poll::Ready(Ok(0)) => {
                    ret = Err(Error::new(
                        ErrorKind::WriteZero,
                        "Failed to write buffered data",
                    ));
                    break;
                }
                Poll::Ready(Ok(n)) => *this.written += n,
                Poll::Ready(Err(ref e)) if e.kind() == ErrorKind::Interrupted => {}
                Poll::Ready(Err(e)) => {
                    ret = Err(e);
                    break;
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        if *this.written > 0 {
            this.buf.drain(..*this.written);
        }
        *this.written = 0;

        Poll::Ready(ret)
    }
}

impl<W: AsyncWrite + fmt::Debug> fmt::Debug for BufferWriter<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufferWriter")
            .field("writer", &self.inner)
            .field("buf", &self.buf)
            .finish()
    }
}

impl<W: AsyncWrite> AsyncWrite for BufferWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        if self.buf.len() + buf.len() > self.buf.capacity() {
            ready!(self.as_mut().poll_flush_buf(cx))?;
        }
        if buf.len() >= self.buf.capacity() {
            self.get_pin_mut().poll_write(cx, buf)
        } else {
            Pin::new(&mut *self.project().buf).poll_write(cx, buf)
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        self.get_pin_mut().poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        self.get_pin_mut().poll_close(cx)
    }
}

impl<W: AsyncWrite + AsyncSeek> AsyncSeek for BufferWriter<W> {
    fn poll_seek(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        pos: SeekFrom,
    ) -> Poll<Result<u64>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        self.get_pin_mut().poll_seek(cx, pos)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct AssertAsync<T>(T);

impl<T> Unpin for AssertAsync<T> {}

impl<T> AssertAsync<T> {
    pub fn new(io: T) -> Self {
        AssertAsync(io)
    }

    pub fn get_ref(&self) -> &T {
        &self.0
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.0
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: std::io::Read> AsyncRead for AssertAsync<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        loop {
            match self.0.read(buf) {
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                res => return Poll::Ready(res),
            }
        }
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        loop {
            match self.0.read_vectored(bufs) {
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                res => return Poll::Ready(res),
            }
        }
    }
}

impl<T: std::io::Write> AsyncWrite for AssertAsync<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        loop {
            match self.0.write(buf) {
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                res => return Poll::Ready(res),
            }
        }
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize>> {
        loop {
            match self.0.write_vectored(bufs) {
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                res => return Poll::Ready(res),
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<()>> {
        loop {
            match self.0.flush() {
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                res => return Poll::Ready(res),
            }
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.poll_flush(cx)
    }
}

impl<T: std::io::Seek> AsyncSeek for AssertAsync<T> {
    fn poll_seek(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        pos: SeekFrom,
    ) -> Poll<Result<u64>> {
        loop {
            match self.0.seek(pos) {
                Err(err) if err.kind() == ErrorKind::Interrupted => {}
                res => return Poll::Ready(res),
            }
        }
    }
}

#[derive(Debug)]
pub struct BlockOn<T>(T);

impl<T> BlockOn<T> {
    pub fn new(io: T) -> BlockOn<T> {
        BlockOn(io)
    }

    pub fn get_ref(&self) -> &T {
        &self.0
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.0
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T: AsyncRead + Unpin> std::io::Read for BlockOn<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        crate::future::block_on(self.0.read(buf))
    }
}

impl<T: AsyncWrite + Unpin> std::io::Write for BlockOn<T> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        crate::future::block_on(self.0.write(buf))
    }

    fn flush(&mut self) -> Result<()> {
        crate::future::block_on(self.0.flush())
    }
}

impl<T: AsyncSeek + Unpin> std::io::Seek for BlockOn<T> {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        crate::future::block_on(self.0.seek(pos))
    }
}

pub fn empty() -> Empty {
    Empty { _private: () }
}

pub struct Empty {
    _private: (),
}

impl fmt::Debug for Empty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("Empty { .. }")
    }
}

impl AsyncRead for Empty {
    #[inline]
    fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut [u8]) -> Poll<Result<usize>> {
        Poll::Ready(Ok(0))
    }
}

impl AsyncBufRead for Empty {
    #[inline]
    fn poll_fill_buf<'a>(self: Pin<&'a mut Self>, _: &mut Context<'_>) -> Poll<Result<&'a [u8]>> {
        Poll::Ready(Ok(&[]))
    }

    #[inline]
    fn consume(self: Pin<&mut Self>, _: usize) {}
}

pub fn sink() -> Sink {
    Sink { _private: () }
}

#[derive(Debug)]
pub struct Sink {
    _private: (),
}

impl AsyncWrite for Sink {
    #[inline]
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    #[inline]
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    #[inline]
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub trait AsyncBufReadExt: AsyncBufRead {
    fn fill_buf(&mut self) -> FillBuf<'_, Self>
    where
        Self: Unpin,
    {
        FillBuf { reader: Some(self) }
    }

    fn consume(&mut self, length: usize)
    where
        Self: Unpin,
    {
        AsyncBufRead::consume(Pin::new(self), length);
    }

    fn read_until<'a>(&'a mut self, byte: u8, buf: &'a mut Vec<u8>) -> ReadUntilFuture<'_, Self>
    where
        Self: Unpin,
    {
        ReadUntilFuture {
            reader: self,
            byte,
            buf,
            read: 0,
        }
    }

    fn read_line<'a>(&'a mut self, buf: &'a mut String) -> ReadLineFuture<'_, Self>
    where
        Self: Unpin,
    {
        ReadLineFuture {
            reader: self,
            buf,
            bytes: Vec::new(),
            read: 0,
        }
    }

    fn lines(self) -> Lines<Self>
    where
        Self: Unpin + Sized,
    {
        Lines {
            reader: self,
            buf: String::new(),
            bytes: Vec::new(),
            read: 0,
        }
    }

    fn split(self, byte: u8) -> Split<Self>
    where
        Self: Sized,
    {
        Split {
            reader: self,
            buf: Vec::new(),
            delim: byte,
            read: 0,
        }
    }
}

impl<R: AsyncBufRead + ?Sized> AsyncBufReadExt for R {}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct FillBuf<'a, R: ?Sized> {
    reader: Option<&'a mut R>,
}

impl<R: ?Sized> Unpin for FillBuf<'_, R> {}

impl<'a, R> Future for FillBuf<'a, R>
where
    R: AsyncBufRead + Unpin + ?Sized,
{
    type Output = Result<&'a [u8]>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        let reader = this
            .reader
            .take()
            .expect("polled `FillBuf` after completion");

        match Pin::new(&mut *reader).poll_fill_buf(cx) {
            Poll::Ready(Ok(_)) => match Pin::new(reader).poll_fill_buf(cx) {
                Poll::Ready(Ok(slice)) => Poll::Ready(Ok(slice)),
                poll => panic!("`poll_fill_buf()` was ready but now it isn't: {:?}", poll),
            },
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => {
                this.reader = Some(reader);
                Poll::Pending
            }
        }
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct ReadUntilFuture<'a, R: Unpin + ?Sized> {
    reader: &'a mut R,
    byte: u8,
    buf: &'a mut Vec<u8>,
    read: usize,
}

impl<R: Unpin + ?Sized> Unpin for ReadUntilFuture<'_, R> {}

impl<R: AsyncBufRead + Unpin + ?Sized> Future for ReadUntilFuture<'_, R> {
    type Output = Result<usize>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self {
            reader,
            byte,
            buf,
            read,
        } = &mut *self;
        read_until_internal(Pin::new(reader), cx, *byte, buf, read)
    }
}

fn read_until_internal<R: AsyncBufReadExt + ?Sized>(
    mut reader: Pin<&mut R>,
    cx: &mut Context<'_>,
    byte: u8,
    buf: &mut Vec<u8>,
    read: &mut usize,
) -> Poll<Result<usize>> {
    loop {
        let (done, used) = {
            let available = ready!(reader.as_mut().poll_fill_buf(cx))?;

            if let Some(i) = memchr::memchr(byte, available) {
                buf.extend_from_slice(&available[..=i]);
                (true, i + 1)
            } else {
                buf.extend_from_slice(available);
                (false, available.len())
            }
        };

        reader.as_mut().consume(used);
        *read += used;

        if done || used == 0 {
            return Poll::Ready(Ok(mem::replace(read, 0)));
        }
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct ReadLineFuture<'a, R: Unpin + ?Sized> {
    reader: &'a mut R,
    buf: &'a mut String,
    bytes: Vec<u8>,
    read: usize,
}

impl<R: Unpin + ?Sized> Unpin for ReadLineFuture<'_, R> {}

impl<R: AsyncBufRead + Unpin + ?Sized> Future for ReadLineFuture<'_, R> {
    type Output = Result<usize>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self {
            reader,
            buf,
            bytes,
            read,
        } = &mut *self;
        read_line_internal(Pin::new(reader), cx, buf, bytes, read)
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "streams do nothing unless polled"]
    pub struct Lines<R> {
        #[pin]
        reader: R,
        buf: String,
        bytes: Vec<u8>,
        read: usize,
    }
}

impl<R: AsyncBufRead> Stream for Lines<R> {
    type Item = Result<String>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        let n = ready!(read_line_internal(
            this.reader,
            cx,
            this.buf,
            this.bytes,
            this.read
        ))?;
        if n == 0 && this.buf.is_empty() {
            return Poll::Ready(None);
        }

        if this.buf.ends_with('\n') {
            this.buf.pop();
            if this.buf.ends_with('\r') {
                this.buf.pop();
            }
        }
        Poll::Ready(Some(Ok(mem::replace(this.buf, String::new()))))
    }
}

fn read_line_internal<R: AsyncBufRead + ?Sized>(
    reader: Pin<&mut R>,
    cx: &mut Context<'_>,
    buf: &mut String,
    bytes: &mut Vec<u8>,
    read: &mut usize,
) -> Poll<Result<usize>> {
    let ret = ready!(read_until_internal(reader, cx, b'\n', bytes, read));

    match String::from_utf8(mem::replace(bytes, Vec::new())) {
        Ok(s) => {
            debug_assert!(buf.is_empty());
            debug_assert_eq!(*read, 0);
            *buf = s;
            Poll::Ready(ret)
        }
        Err(_) => Poll::Ready(ret.and_then(|_| {
            Err(Error::new(
                ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            ))
        })),
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "streams do nothing unless polled"]
    pub struct Split<R> {
        #[pin]
        reader: R,
        buf: Vec<u8>,
        read: usize,
        delim: u8,
    }
}

impl<R: AsyncBufRead> Stream for Split<R> {
    type Item = Result<Vec<u8>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        let n = ready!(read_until_internal(
            this.reader,
            cx,
            *this.delim,
            this.buf,
            this.read
        ))?;
        if n == 0 && this.buf.is_empty() {
            return Poll::Ready(None);
        }

        if this.buf[this.buf.len() - 1] == *this.delim {
            this.buf.pop();
        }
        Poll::Ready(Some(Ok(mem::replace(this.buf, vec![]))))
    }
}

pub trait AsyncReadExt: AsyncRead {
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> ReadFuture<'a, Self>
    where
        Self: Unpin,
    {
        ReadFuture { reader: self, buf }
    }

    fn read_vectored<'a>(
        &'a mut self,
        bufs: &'a mut [IoSliceMut<'a>],
    ) -> ReadVectoredFuture<'a, Self>
    where
        Self: Unpin,
    {
        ReadVectoredFuture { reader: self, bufs }
    }

    fn read_to_end<'a>(&'a mut self, buf: &'a mut Vec<u8>) -> ReadToEndFuture<'a, Self>
    where
        Self: Unpin,
    {
        let start_len = buf.len();
        ReadToEndFuture {
            reader: self,
            buf,
            start_len,
        }
    }

    fn read_to_string<'a>(&'a mut self, buf: &'a mut String) -> ReadToStringFuture<'a, Self>
    where
        Self: Unpin,
    {
        ReadToStringFuture {
            reader: self,
            buf,
            bytes: Vec::new(),
            start_len: 0,
        }
    }

    fn read_exact<'a>(&'a mut self, buf: &'a mut [u8]) -> ReadExactFuture<'a, Self>
    where
        Self: Unpin,
    {
        ReadExactFuture { reader: self, buf }
    }

    fn take(self, limit: u64) -> Take<Self>
    where
        Self: Sized,
    {
        Take { inner: self, limit }
    }

    fn bytes(self) -> Bytes<Self>
    where
        Self: Sized,
    {
        Bytes { inner: self }
    }

    fn chain<R: AsyncRead>(self, next: R) -> Chain<Self, R>
    where
        Self: Sized,
    {
        Chain {
            first: self,
            second: next,
            done_first: false,
        }
    }

    #[cfg(feature = "alloc")]
    fn boxed_reader<'a>(self) -> Pin<Box<dyn AsyncRead + Send + 'a>>
    where
        Self: Sized + Send + 'a,
    {
        Box::pin(self)
    }
}

impl<R: AsyncRead + ?Sized> AsyncReadExt for R {}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct ReadFuture<'a, R: Unpin + ?Sized> {
    reader: &'a mut R,
    buf: &'a mut [u8],
}

impl<R: Unpin + ?Sized> Unpin for ReadFuture<'_, R> {}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadFuture<'_, R> {
    type Output = Result<usize>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { reader, buf } = &mut *self;
        Pin::new(reader).poll_read(cx, buf)
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct ReadVectoredFuture<'a, R: Unpin + ?Sized> {
    reader: &'a mut R,
    bufs: &'a mut [IoSliceMut<'a>],
}

impl<R: Unpin + ?Sized> Unpin for ReadVectoredFuture<'_, R> {}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadVectoredFuture<'_, R> {
    type Output = Result<usize>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { reader, bufs } = &mut *self;
        Pin::new(reader).poll_read_vectored(cx, bufs)
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct ReadToEndFuture<'a, R: Unpin + ?Sized> {
    reader: &'a mut R,
    buf: &'a mut Vec<u8>,
    start_len: usize,
}

impl<R: Unpin + ?Sized> Unpin for ReadToEndFuture<'_, R> {}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadToEndFuture<'_, R> {
    type Output = Result<usize>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self {
            reader,
            buf,
            start_len,
        } = &mut *self;
        read_to_end_internal(Pin::new(reader), cx, buf, *start_len)
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct ReadToStringFuture<'a, R: Unpin + ?Sized> {
    reader: &'a mut R,
    buf: &'a mut String,
    bytes: Vec<u8>,
    start_len: usize,
}

impl<R: Unpin + ?Sized> Unpin for ReadToStringFuture<'_, R> {}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadToStringFuture<'_, R> {
    type Output = Result<usize>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self {
            reader,
            buf,
            bytes,
            start_len,
        } = &mut *self;
        let reader = Pin::new(reader);

        let ret = ready!(read_to_end_internal(reader, cx, bytes, *start_len));

        match String::from_utf8(mem::replace(bytes, Vec::new())) {
            Ok(s) => {
                debug_assert!(buf.is_empty());
                **buf = s;
                Poll::Ready(ret)
            }
            Err(_) => Poll::Ready(ret.and_then(|_| {
                Err(Error::new(
                    ErrorKind::InvalidData,
                    "stream did not contain valid UTF-8",
                ))
            })),
        }
    }
}

fn read_to_end_internal<R: AsyncRead + ?Sized>(
    mut rd: Pin<&mut R>,
    cx: &mut Context<'_>,
    buf: &mut Vec<u8>,
    start_len: usize,
) -> Poll<Result<usize>> {
    struct Guard<'a> {
        buf: &'a mut Vec<u8>,
        len: usize,
    }

    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            self.buf.resize(self.len, 0);
        }
    }

    let mut g = Guard {
        len: buf.len(),
        buf,
    };
    let ret;
    loop {
        if g.len == g.buf.len() {
            g.buf.reserve(32);
            let capacity = g.buf.capacity();
            g.buf.resize(capacity, 0);
        }

        match ready!(rd.as_mut().poll_read(cx, &mut g.buf[g.len..])) {
            Ok(0) => {
                ret = Poll::Ready(Ok(g.len - start_len));
                break;
            }
            Ok(n) => g.len += n,
            Err(e) => {
                ret = Poll::Ready(Err(e));
                break;
            }
        }
    }

    ret
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct ReadExactFuture<'a, R: Unpin + ?Sized> {
    reader: &'a mut R,
    buf: &'a mut [u8],
}

impl<R: Unpin + ?Sized> Unpin for ReadExactFuture<'_, R> {}

impl<R: AsyncRead + Unpin + ?Sized> Future for ReadExactFuture<'_, R> {
    type Output = Result<()>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { reader, buf } = &mut *self;
        while !buf.is_empty() {
            let n = crate::ready!(Pin::new(&mut *reader).poll_read(cx, buf))?;
            let (_, rest) = mem::replace(buf, &mut []).split_at_mut(n);
            *buf = rest;

            if n == 0 {
                return Poll::Ready(Err(ErrorKind::UnexpectedEof.into()));
            }
        }

        Poll::Ready(Ok(()))
    }
}

pin_project! {
    #[derive(Debug)]
    pub struct Take<R> {
        #[pin]
        inner: R,
        limit: u64,
    }
}

impl<R> Take<R> {
    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn set_limit(&mut self, limit: u64) {
        self.limit = limit;
    }

    pub fn get_ref(&self) -> &R {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: AsyncRead> AsyncRead for Take<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        let this = self.project();
        take_read_internal(this.inner, cx, buf, this.limit)
    }
}

fn take_read_internal<R: AsyncRead + ?Sized>(
    mut rd: Pin<&mut R>,
    cx: &mut Context<'_>,
    buf: &mut [u8],
    limit: &mut u64,
) -> Poll<Result<usize>> {
    if *limit == 0 {
        return Poll::Ready(Ok(0));
    }

    let max = cmp::min(buf.len() as u64, *limit) as usize;

    match ready!(rd.as_mut().poll_read(cx, &mut buf[..max])) {
        Ok(n) => {
            *limit -= n as u64;
            Poll::Ready(Ok(n))
        }
        Err(e) => Poll::Ready(Err(e)),
    }
}

impl<R: AsyncBufRead> AsyncBufRead for Take<R> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<&[u8]>> {
        let this = self.project();

        if *this.limit == 0 {
            return Poll::Ready(Ok(&[]));
        }

        match ready!(this.inner.poll_fill_buf(cx)) {
            Ok(buf) => {
                let cap = cmp::min(buf.len() as u64, *this.limit) as usize;
                Poll::Ready(Ok(&buf[..cap]))
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn consume(self: Pin<&mut Self>, length: usize) {
        let this = self.project();
        let length = cmp::min(length as u64, *this.limit) as usize;
        *this.limit -= length as u64;

        this.inner.consume(length);
    }
}

pin_project! {
    #[derive(Debug)]
    pub struct Bytes<R> {
        #[pin]
        inner: R,
    }
}

impl<R: AsyncRead + Unpin> Stream for Bytes<R> {
    type Item = Result<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut byte = 0;

        let rd = Pin::new(&mut self.inner);

        match ready!(rd.poll_read(cx, std::slice::from_mut(&mut byte))) {
            Ok(0) => Poll::Ready(None),
            Ok(..) => Poll::Ready(Some(Ok(byte))),
            Err(ref e) if e.kind() == ErrorKind::Interrupted => Poll::Pending,
            Err(e) => Poll::Ready(Some(Err(e))),
        }
    }
}

impl<R: AsyncRead> AsyncRead for Bytes<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        self.project().inner.poll_read(cx, buf)
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        self.project().inner.poll_read_vectored(cx, bufs)
    }
}

pin_project! {
    pub struct Chain<R1, R2> {
        #[pin]
        first: R1,
        #[pin]
        second: R2,
        done_first: bool,
    }
}

impl<R1, R2> Chain<R1, R2> {
    pub fn get_ref(&self) -> (&R1, &R2) {
        (&self.first, &self.second)
    }

    pub fn get_mut(&mut self) -> (&mut R1, &mut R2) {
        (&mut self.first, &mut self.second)
    }

    pub fn into_inner(self) -> (R1, R2) {
        (self.first, self.second)
    }
}

impl<R1: fmt::Debug, R2: fmt::Debug> fmt::Debug for Chain<R1, R2> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chain")
            .field("r1", &self.first)
            .field("r2", &self.second)
            .finish()
    }
}

impl<R1: AsyncRead, R2: AsyncRead> AsyncRead for Chain<R1, R2> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        let this = self.project();
        if !*this.done_first {
            match ready!(this.first.poll_read(cx, buf)) {
                Ok(0) if !buf.is_empty() => *this.done_first = true,
                Ok(n) => return Poll::Ready(Ok(n)),
                Err(err) => return Poll::Ready(Err(err)),
            }
        }

        this.second.poll_read(cx, buf)
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        let this = self.project();
        if !*this.done_first {
            match ready!(this.first.poll_read_vectored(cx, bufs)) {
                Ok(0) if !bufs.is_empty() => *this.done_first = true,
                Ok(n) => return Poll::Ready(Ok(n)),
                Err(err) => return Poll::Ready(Err(err)),
            }
        }

        this.second.poll_read_vectored(cx, bufs)
    }
}

impl<R1: AsyncBufRead, R2: AsyncBufRead> AsyncBufRead for Chain<R1, R2> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<&[u8]>> {
        let this = self.project();
        if !*this.done_first {
            match ready!(this.first.poll_fill_buf(cx)) {
                Ok(buf) if buf.is_empty() => {
                    *this.done_first = true;
                }
                Ok(buf) => return Poll::Ready(Ok(buf)),
                Err(err) => return Poll::Ready(Err(err)),
            }
        }

        this.second.poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, length: usize) {
        let this = self.project();
        if !*this.done_first {
            this.first.consume(length)
        } else {
            this.second.consume(length)
        }
    }
}

pub trait AsyncSeekExt: AsyncSeek {
    fn seek(&mut self, pos: SeekFrom) -> SeekFuture<'_, Self>
    where
        Self: Unpin,
    {
        SeekFuture { seeker: self, pos }
    }
}

impl<S: AsyncSeek + ?Sized> AsyncSeekExt for S {}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct SeekFuture<'a, S: Unpin + ?Sized> {
    seeker: &'a mut S,
    pos: SeekFrom,
}

impl<S: Unpin + ?Sized> Unpin for SeekFuture<'_, S> {}

impl<S: AsyncSeek + Unpin + ?Sized> Future for SeekFuture<'_, S> {
    type Output = Result<u64>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let pos = self.pos;
        Pin::new(&mut *self.seeker).poll_seek(cx, pos)
    }
}

pub trait AsyncWriteExt: AsyncWrite {
    fn write<'a>(&'a mut self, buf: &'a [u8]) -> WriteFuture<'a, Self>
    where
        Self: Unpin,
    {
        WriteFuture { writer: self, buf }
    }

    fn write_vectored<'a>(&'a mut self, bufs: &'a [IoSlice<'a>]) -> WriteVectoredFuture<'a, Self>
    where
        Self: Unpin,
    {
        WriteVectoredFuture { writer: self, bufs }
    }

    fn write_all<'a>(&'a mut self, buf: &'a [u8]) -> WriteAllFuture<'a, Self>
    where
        Self: Unpin,
    {
        WriteAllFuture { writer: self, buf }
    }

    fn flush(&mut self) -> FlushFuture<'_, Self>
    where
        Self: Unpin,
    {
        FlushFuture { writer: self }
    }

    fn close(&mut self) -> CloseFuture<'_, Self>
    where
        Self: Unpin,
    {
        CloseFuture { writer: self }
    }

    #[cfg(feature = "alloc")]
    fn boxed_writer<'a>(self) -> Pin<Box<dyn AsyncWrite + Send + 'a>>
    where
        Self: Sized + Send + 'a,
    {
        Box::pin(self)
    }
}

impl<W: AsyncWrite + ?Sized> AsyncWriteExt for W {}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct WriteFuture<'a, W: Unpin + ?Sized> {
    writer: &'a mut W,
    buf: &'a [u8],
}

impl<W: Unpin + ?Sized> Unpin for WriteFuture<'_, W> {}

impl<W: AsyncWrite + Unpin + ?Sized> Future for WriteFuture<'_, W> {
    type Output = Result<usize>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let buf = self.buf;
        Pin::new(&mut *self.writer).poll_write(cx, buf)
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct WriteVectoredFuture<'a, W: Unpin + ?Sized> {
    writer: &'a mut W,
    bufs: &'a [IoSlice<'a>],
}

impl<W: Unpin + ?Sized> Unpin for WriteVectoredFuture<'_, W> {}

impl<W: AsyncWrite + Unpin + ?Sized> Future for WriteVectoredFuture<'_, W> {
    type Output = Result<usize>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let bufs = self.bufs;
        Pin::new(&mut *self.writer).poll_write_vectored(cx, bufs)
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct WriteAllFuture<'a, W: Unpin + ?Sized> {
    writer: &'a mut W,
    buf: &'a [u8],
}

impl<W: Unpin + ?Sized> Unpin for WriteAllFuture<'_, W> {}

impl<W: AsyncWrite + Unpin + ?Sized> Future for WriteAllFuture<'_, W> {
    type Output = Result<()>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self { writer, buf } = &mut *self;
        while !buf.is_empty() {
            let n = ready!(Pin::new(&mut **writer).poll_write(cx, buf))?;
            let (_, rest) = mem::replace(buf, &[]).split_at(n);
            *buf = rest;

            if n == 0 {
                return Poll::Ready(Err(ErrorKind::WriteZero.into()));
            }
        }

        Poll::Ready(Ok(()))
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct FlushFuture<'a, W: Unpin + ?Sized> {
    writer: &'a mut W,
}

impl<W: Unpin + ?Sized> Unpin for FlushFuture<'_, W> {}

impl<W: AsyncWrite + Unpin + ?Sized> Future for FlushFuture<'_, W> {
    type Output = Result<()>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.writer).poll_flush(cx)
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct CloseFuture<'a, W: Unpin + ?Sized> {
    writer: &'a mut W,
}

impl<W: Unpin + ?Sized> Unpin for CloseFuture<'_, W> {}

impl<W: AsyncWrite + Unpin + ?Sized> Future for CloseFuture<'_, W> {
    type Output = Result<()>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut *self.writer).poll_close(cx)
    }
}

#[cfg(feature = "alloc")]
pub type BoxedReader = Pin<Box<dyn AsyncRead + Send + 'static>>;

#[cfg(feature = "alloc")]
pub type BoxedWriter = Pin<Box<dyn AsyncWrite + Send + 'static>>;

pub fn split<T>(stream: T) -> (ReadHalf<T>, WriteHalf<T>)
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let inner = Arc::new(Mutex::new(stream));
    (ReadHalf(inner.clone()), WriteHalf(inner))
}

#[derive(Debug)]
pub struct ReadHalf<T>(Arc<Mutex<T>>);

#[derive(Debug)]
pub struct WriteHalf<T>(Arc<Mutex<T>>);

impl<T: AsyncRead + Unpin> AsyncRead for ReadHalf<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        let mut inner = self.0.lock().unwrap();
        Pin::new(&mut *inner).poll_read(cx, buf)
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        let mut inner = self.0.lock().unwrap();
        Pin::new(&mut *inner).poll_read_vectored(cx, bufs)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for WriteHalf<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
        let mut inner = self.0.lock().unwrap();
        Pin::new(&mut *inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let mut inner = self.0.lock().unwrap();
        Pin::new(&mut *inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let mut inner = self.0.lock().unwrap();
        Pin::new(&mut *inner).poll_close(cx)
    }
}

pub async fn copy<R, W>(reader: R, writer: W) -> Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    pin_project! {
        struct CopyFuture<R, W> {
            #[pin]
            reader: R,
            #[pin]
            writer: W,
            amt: u64,
        }
    }

    impl<R, W> Future for CopyFuture<R, W>
    where
        R: AsyncBufRead,
        W: AsyncWrite + Unpin,
    {
        type Output = Result<u64>;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let mut this = self.project();
            loop {
                let buffer = ready!(this.reader.as_mut().poll_fill_buf(cx))?;
                if buffer.is_empty() {
                    ready!(this.writer.as_mut().poll_flush(cx))?;
                    return Poll::Ready(Ok(*this.amt));
                }

                let i = ready!(this.writer.as_mut().poll_write(cx, buffer))?;
                if i == 0 {
                    return Poll::Ready(Err(ErrorKind::WriteZero.into()));
                }
                *this.amt += i as u64;
                this.reader.as_mut().consume(i);
            }
        }
    }

    let future = CopyFuture {
        reader: BufferReader::new(reader),
        writer,
        amt: 0,
    };
    future.await
}
