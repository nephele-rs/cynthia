use std::convert::TryFrom;
use std::fmt;
use std::io::{self, Read as _, Write as _};
use std::net::Shutdown;

#[cfg(unix)]
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, RawSocket};

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::os::unix::net::{self, SocketAddr};

use crate::future::prelude::*;
use crate::io::Async;
use crate::ready;

#[derive(Clone, Debug)]
pub struct UnixListener {
    inner: Arc<Async<net::UnixListener>>,
}

impl UnixListener {
    fn new(inner: Arc<Async<net::UnixListener>>) -> UnixListener {
        UnixListener { inner }
    }

    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<UnixListener> {
        let listener = Async::<net::UnixListener>::bind(path)?;
        Ok(UnixListener::new(Arc::new(listener)))
    }

    pub async fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
        let (stream, addr) = self.inner.accept().await?;
        Ok((UnixStream::new(Arc::new(stream)), addr))
    }

    pub fn incoming(&self) -> Incoming<'_> {
        Incoming {
            listener: self,
            accept: None,
        }
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.get_ref().set_nonblocking(nonblocking)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }
}

impl From<Async<net::UnixListener>> for UnixListener {
    fn from(listener: Async<net::UnixListener>) -> UnixListener {
        UnixListener::new(Arc::new(listener))
    }
}

impl TryFrom<net::UnixListener> for UnixListener {
    type Error = io::Error;

    fn try_from(listener: net::UnixListener) -> io::Result<UnixListener> {
        Ok(UnixListener::new(Arc::new(Async::new(listener)?)))
    }
}

impl Into<Arc<Async<net::UnixListener>>> for UnixListener {
    fn into(self) -> Arc<Async<net::UnixListener>> {
        self.inner
    }
}

#[cfg(unix)]
impl AsRawFd for UnixListener {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for UnixListener {
    fn as_raw_socket(&self) -> RawSocket {
        self.inner.as_raw_socket()
    }
}

pub struct Incoming<'a> {
    listener: &'a UnixListener,
    accept: Option<
        Pin<Box<dyn Future<Output = io::Result<(UnixStream, SocketAddr)>> + Send + Sync + 'a>>,
    >,
}

impl Stream for Incoming<'_> {
    type Item = io::Result<UnixStream>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if self.accept.is_none() {
                self.accept = Some(Box::pin(self.listener.accept()));
            }

            if let Some(f) = &mut self.accept {
                let res = ready!(f.as_mut().poll(cx));
                self.accept = None;
                return Poll::Ready(Some(res.map(|(stream, _)| stream)));
            }
        }
    }
}

impl fmt::Debug for Incoming<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Incoming")
            .field("listener", self.listener)
            .finish()
    }
}

pub struct UnixStream {
    inner: Arc<Async<net::UnixStream>>,
    readable: Option<Pin<Box<dyn Future<Output = io::Result<()>> + Send + Sync>>>,
    writable: Option<Pin<Box<dyn Future<Output = io::Result<()>> + Send + Sync>>>,
}

impl UnwindSafe for UnixStream {}
impl RefUnwindSafe for UnixStream {}

impl UnixStream {
    fn new(inner: Arc<Async<net::UnixStream>>) -> UnixStream {
        UnixStream {
            inner,
            readable: None,
            writable: None,
        }
    }

    pub async fn connect<P: AsRef<Path>>(path: P) -> io::Result<UnixStream> {
        let stream = Async::<net::UnixStream>::connect(path).await?;
        Ok(UnixStream::new(Arc::new(stream)))
    }

    pub fn pair() -> io::Result<(UnixStream, UnixStream)> {
        let (a, b) = Async::<net::UnixStream>::pair()?;
        Ok((UnixStream::new(Arc::new(a)), UnixStream::new(Arc::new(b))))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.inner.get_ref().shutdown(how)
    }
}

impl fmt::Debug for UnixStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl Clone for UnixStream {
    fn clone(&self) -> UnixStream {
        UnixStream::new(self.inner.clone())
    }
}

impl From<Async<net::UnixStream>> for UnixStream {
    fn from(stream: Async<net::UnixStream>) -> UnixStream {
        UnixStream::new(Arc::new(stream))
    }
}

impl TryFrom<net::UnixStream> for UnixStream {
    type Error = io::Error;

    fn try_from(stream: net::UnixStream) -> io::Result<UnixStream> {
        Ok(UnixStream::new(Arc::new(Async::new(stream)?)))
    }
}

impl Into<Arc<Async<net::UnixStream>>> for UnixStream {
    fn into(self) -> Arc<Async<net::UnixStream>> {
        self.inner
    }
}

#[cfg(unix)]
impl AsRawFd for UnixStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for UnixStream {
    fn as_raw_socket(&self) -> RawSocket {
        self.inner.as_raw_socket()
    }
}

impl AsyncRead for UnixStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            match self.inner.get_ref().read(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => {
                    self.readable = None;
                    return Poll::Ready(res);
                }
            }

            if self.readable.is_none() {
                let inner = self.inner.clone();
                self.readable = Some(Box::pin(async move { inner.readable().await }));
            }

            if let Some(f) = &mut self.readable {
                let res = ready!(f.as_mut().poll(cx));
                self.readable = None;
                res?;
            }
        }
    }
}

impl AsyncWrite for UnixStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            match self.inner.get_ref().write(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => {
                    self.writable = None;
                    return Poll::Ready(res);
                }
            }

            if self.writable.is_none() {
                let inner = self.inner.clone();
                self.writable = Some(Box::pin(async move { inner.writable().await }));
            }

            if let Some(f) = &mut self.writable {
                let res = ready!(f.as_mut().poll(cx));
                self.writable = None;
                res?;
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            match self.inner.get_ref().flush() {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => {
                    self.writable = None;
                    return Poll::Ready(res);
                }
            }

            if self.writable.is_none() {
                let inner = self.inner.clone();
                self.writable = Some(Box::pin(async move { inner.writable().await }));
            }

            if let Some(f) = &mut self.writable {
                let res = ready!(f.as_mut().poll(cx));
                self.writable = None;
                res?;
            }
        }
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.inner.get_ref().shutdown(Shutdown::Write))
    }
}

#[derive(Clone, Debug)]
pub struct UnixDatagram {
    inner: Arc<Async<net::UnixDatagram>>,
}

impl UnixDatagram {
    fn new(inner: Arc<Async<net::UnixDatagram>>) -> UnixDatagram {
        UnixDatagram { inner }
    }

    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<UnixDatagram> {
        let socket = Async::<net::UnixDatagram>::bind(path)?;
        Ok(UnixDatagram::new(Arc::new(socket)))
    }

    pub fn unbound() -> io::Result<UnixDatagram> {
        let socket = Async::<net::UnixDatagram>::unbound()?;
        Ok(UnixDatagram::new(Arc::new(socket)))
    }

    pub fn pair() -> io::Result<(UnixDatagram, UnixDatagram)> {
        let (a, b) = Async::<net::UnixDatagram>::pair()?;
        Ok((
            UnixDatagram::new(Arc::new(a)),
            UnixDatagram::new(Arc::new(b)),
        ))
    }

    pub fn connect<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let p = path.as_ref();
        self.inner.get_ref().connect(p)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.recv_from(buf).await
    }

    pub async fn send_to<P: AsRef<Path>>(&self, buf: &[u8], path: P) -> io::Result<usize> {
        self.inner.send_to(buf, path.as_ref()).await
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.recv(buf).await
    }

    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.inner.send(buf).await
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.inner.get_ref().shutdown(how)
    }
}

#[cfg(unix)]
impl AsRawFd for UnixDatagram {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for UnixDatagram {
    fn as_raw_socket(&self) -> RawSocket {
        self.inner.as_raw_socket()
    }
}

impl From<Async<net::UnixDatagram>> for UnixDatagram {
    fn from(socket: Async<net::UnixDatagram>) -> UnixDatagram {
        UnixDatagram::new(Arc::new(socket))
    }
}

impl TryFrom<net::UnixDatagram> for UnixDatagram {
    type Error = io::Error;

    fn try_from(socket: net::UnixDatagram) -> io::Result<UnixDatagram> {
        Ok(UnixDatagram::new(Arc::new(Async::new(socket)?)))
    }
}

impl Into<Arc<Async<net::UnixDatagram>>> for UnixDatagram {
    fn into(self) -> Arc<Async<net::UnixDatagram>> {
        self.inner
    }
}
