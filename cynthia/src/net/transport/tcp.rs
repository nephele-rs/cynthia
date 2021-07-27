use std::convert::TryFrom;
use std::fmt;
use std::io::{self, Read as _, Write as _};
use std::net::{Shutdown, SocketAddr};

#[cfg(unix)]
use std::os::unix::io::{AsRawFd, RawFd};

#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, RawSocket};

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::io::Async;
use crate::net::transport::addr::AsyncToSocketAddrs;
use crate::{future::prelude::*, ready};

#[derive(Clone, Debug)]
pub struct TcpListener {
    inner: Arc<Async<std::net::TcpListener>>,
}

impl TcpListener {
    fn new(inner: Arc<Async<std::net::TcpListener>>) -> TcpListener {
        TcpListener { inner }
    }

    pub async fn bind<A: AsyncToSocketAddrs>(addr: A) -> io::Result<TcpListener> {
        let mut last_err = None;

        for addr in addr.to_socket_addrs().await? {
            match Async::<std::net::TcpListener>::bind_addr(addr) {
                Ok(listener) => return Ok(TcpListener::new(Arc::new(listener))),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "addresses resolve failed")
        }))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let (stream, addr) = self.inner.accept().await?;
        Ok((TcpStream::new(Arc::new(stream)), addr))
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

    pub fn ttl(&self) -> io::Result<u32> {
        self.inner.get_ref().ttl()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.get_ref().set_ttl(ttl)
    }
}

impl From<Async<std::net::TcpListener>> for TcpListener {
    fn from(listener: Async<std::net::TcpListener>) -> TcpListener {
        TcpListener::new(Arc::new(listener))
    }
}

impl TryFrom<std::net::TcpListener> for TcpListener {
    type Error = io::Error;

    fn try_from(listener: std::net::TcpListener) -> io::Result<TcpListener> {
        Ok(TcpListener::new(Arc::new(Async::new(listener)?)))
    }
}

impl Into<Arc<Async<std::net::TcpListener>>> for TcpListener {
    fn into(self) -> Arc<Async<std::net::TcpListener>> {
        self.inner
    }
}

#[cfg(unix)]
impl AsRawFd for TcpListener {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for TcpListener {
    fn as_raw_socket(&self) -> RawSocket {
        self.inner.as_raw_socket()
    }
}

pub struct Incoming<'a> {
    listener: &'a TcpListener,
    accept: Option<
        Pin<Box<dyn Future<Output = io::Result<(TcpStream, SocketAddr)>> + Send + Sync + 'a>>,
    >,
}

impl Stream for Incoming<'_> {
    type Item = io::Result<TcpStream>;

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

pub struct TcpStream {
    inner: Arc<Async<std::net::TcpStream>>,
    readable: Option<Pin<Box<dyn Future<Output = io::Result<()>> + Send + Sync>>>,
    writable: Option<Pin<Box<dyn Future<Output = io::Result<()>> + Send + Sync>>>,
}

impl UnwindSafe for TcpStream {}
impl RefUnwindSafe for TcpStream {}

impl TcpStream {
    fn new(inner: Arc<Async<std::net::TcpStream>>) -> TcpStream {
        TcpStream {
            inner,
            readable: None,
            writable: None,
        }
    }

    pub async fn connect<A: AsyncToSocketAddrs>(addr: A) -> io::Result<TcpStream> {
        let mut last_err = None;

        for addr in addr.to_socket_addrs().await? {
            match Async::<std::net::TcpStream>::connect_addr(addr).await {
                Ok(stream) => return Ok(TcpStream::new(Arc::new(stream))),
                Err(e) => last_err = Some(e),
            }
        }

        Err(last_err
            .unwrap_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "connect failed")))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }

    pub fn shutdown(&self, how: std::net::Shutdown) -> std::io::Result<()> {
        self.inner.get_ref().shutdown(how)
    }

    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.peek(buf).await
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        self.inner.get_ref().nodelay()
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        self.inner.get_ref().set_nodelay(nodelay)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.inner.get_ref().set_nonblocking(nonblocking)
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.inner.get_ref().ttl()
    }

    pub fn set_ttl(&self, ttl: u32) -> io::Result<()> {
        self.inner.get_ref().set_ttl(ttl)
    }
}

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl Clone for TcpStream {
    fn clone(&self) -> TcpStream {
        TcpStream::new(self.inner.clone())
    }
}

impl From<Async<std::net::TcpStream>> for TcpStream {
    fn from(stream: Async<std::net::TcpStream>) -> TcpStream {
        TcpStream::new(Arc::new(stream))
    }
}

impl Into<Arc<Async<std::net::TcpStream>>> for TcpStream {
    fn into(self) -> Arc<Async<std::net::TcpStream>> {
        self.inner
    }
}

impl TryFrom<std::net::TcpStream> for TcpStream {
    type Error = io::Error;

    fn try_from(stream: std::net::TcpStream) -> io::Result<TcpStream> {
        Ok(TcpStream::new(Arc::new(Async::new(stream)?)))
    }
}

#[cfg(unix)]
impl AsRawFd for TcpStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for TcpStream {
    fn as_raw_socket(&self) -> RawSocket {
        self.inner.as_raw_socket()
    }
}

impl AsyncRead for TcpStream {
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

impl AsyncWrite for TcpStream {
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
