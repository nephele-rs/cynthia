use std::convert::TryFrom;
use std::future::Future;
use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::{
    os::unix::io::{AsRawFd, RawFd},
    os::unix::net::{SocketAddr as UnixSocketAddr, UnixDatagram, UnixListener, UnixStream},
    path::Path,
};

#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, RawSocket};

use crate::future::stream::{self, Stream};
use crate::future::swap::{AsyncRead, AsyncWrite};
use crate::io::reactor::{Reactor, Source};
use crate::net::connect::connect;
use crate::{future::future, pin, ready};
use crate::net::transport::{resolve, AsyncToSocketAddrs};

pub mod reactor;
mod worker;

pub use worker::block_on;

mod fd;
pub use fd::TransportFd;

fn duration_max() -> Duration {
    Duration::new(std::u64::MAX, 1_000_000_000 - 1)
}

#[derive(Debug)]
pub struct Timer {
    id_and_waker: Option<(usize, Waker)>,
    when: Instant,
    period: Duration,
}

impl Timer {
    pub fn after(duration: Duration) -> Timer {
        Timer::at(Instant::now() + duration)
    }

    pub fn at(instant: Instant) -> Timer {
        Timer::interval_at(instant, duration_max())
    }

    pub fn interval(period: Duration) -> Timer {
        Timer::interval_at(Instant::now() + period, period)
    }

    pub fn interval_at(start: Instant, period: Duration) -> Timer {
        Timer {
            id_and_waker: None,
            when: start,
            period,
        }
    }

    pub fn set_after(&mut self, duration: Duration) {
        self.set_at(Instant::now() + duration);
    }

    pub fn set_at(&mut self, instant: Instant) {
        if let Some((id, _)) = self.id_and_waker.as_ref() {
            Reactor::get().remove_timer(self.when, *id);
        }

        self.when = instant;

        if let Some((id, waker)) = self.id_and_waker.as_mut() {
            *id = Reactor::get().add_timer(self.when, waker);
        }
    }

    pub fn set_interval(&mut self, period: Duration) {
        self.set_interval_at(Instant::now() + period, period);
    }

    pub fn set_interval_at(&mut self, start: Instant, period: Duration) {
        if let Some((id, _)) = self.id_and_waker.as_ref() {
            Reactor::get().remove_timer(self.when, *id);
        }

        self.when = start;
        self.period = period;

        if let Some((id, waker)) = self.id_and_waker.as_mut() {
            *id = Reactor::get().add_timer(self.when, waker);
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some((id, _)) = self.id_and_waker.take() {
            Reactor::get().remove_timer(self.when, id);
        }
    }
}

impl Future for Timer {
    type Output = Instant;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.poll_next(cx) {
            Poll::Ready(Some(when)) => Poll::Ready(when),
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => unreachable!(),
        }
    }
}

impl Stream for Timer {
    type Item = Instant;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if Instant::now() >= self.when {
            if let Some((id, _)) = self.id_and_waker.take() {
                Reactor::get().remove_timer(self.when, id);
            }
            let when = self.when;
            if let Some(next) = when.checked_add(self.period) {
                self.when = next;
                let id = Reactor::get().add_timer(self.when, cx.waker());
                self.id_and_waker = Some((id, cx.waker().clone()));
            }
            return Poll::Ready(Some(when));
        } else {
            match &self.id_and_waker {
                None => {
                    let id = Reactor::get().add_timer(self.when, cx.waker());
                    self.id_and_waker = Some((id, cx.waker().clone()));
                }
                Some((id, w)) if !w.will_wake(cx.waker()) => {
                    Reactor::get().remove_timer(self.when, *id);

                    let id = Reactor::get().add_timer(self.when, cx.waker());
                    self.id_and_waker = Some((id, cx.waker().clone()));
                }
                Some(_) => {}
            }
        }
        Poll::Pending
    }
}

#[derive(Debug)]
pub struct Async<T> {
    source: Arc<Source>,
    io: Option<T>,
}

impl<T> Unpin for Async<T> {}

#[cfg(unix)]
impl<T: AsRawFd> Async<T> {
    pub fn new(io: T) -> io::Result<Async<T>> {
        let fd = io.as_raw_fd();

        unsafe {
            let mut res = libc::fcntl(fd, libc::F_GETFL);
            if res != -1 {
                res = libc::fcntl(fd, libc::F_SETFL, res | libc::O_NONBLOCK);
            }
            if res == -1 {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(Async {
            source: Reactor::get().add_io(fd)?,
            io: Some(io),
        })
    }
}

#[cfg(unix)]
impl<T: AsRawFd> AsRawFd for Async<T> {
    fn as_raw_fd(&self) -> RawFd {
        self.source.raw
    }
}

#[cfg(windows)]
impl<T: AsRawSocket> Async<T> {
    pub fn new(io: T) -> io::Result<Async<T>> {
        let sock = io.as_raw_socket();

        unsafe {
            use winapi::ctypes;
            use winapi::um::winsock2;

            let mut nonblocking = true as ctypes::c_ulong;
            let res = winsock2::ioctlsocket(
                sock as winsock2::SOCKET,
                winsock2::FIONBIO,
                &mut nonblocking,
            );
            if res != 0 {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(Async {
            source: Reactor::get().add_io(sock)?,
            io: Some(io),
        })
    }
}

#[cfg(windows)]
impl<T: AsRawSocket> AsRawSocket for Async<T> {
    fn as_raw_socket(&self) -> RawSocket {
        self.source.raw
    }
}

impl<T> Async<T> {
    pub fn get_ref(&self) -> &T {
        self.io.as_ref().unwrap()
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.io.as_mut().unwrap()
    }

    pub fn into_inner(mut self) -> io::Result<T> {
        let io = self.io.take().unwrap();
        Reactor::get().remove_io(&self.source)?;
        Ok(io)
    }

    pub async fn readable(&self) -> io::Result<()> {
        self.source.readable().await
    }

    pub async fn writable(&self) -> io::Result<()> {
        self.source.writable().await
    }

    pub fn poll_readable(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.source.poll_readable(cx)
    }

    pub fn poll_writable(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.source.poll_writable(cx)
    }

    pub async fn read_with<R>(&self, op: impl FnMut(&T) -> io::Result<R>) -> io::Result<R> {
        let mut op = op;
        loop {
            match op(self.get_ref()) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return res,
            }
            optimistic(self.readable()).await?;
        }
    }

    pub async fn read_with_mut<R>(
        &mut self,
        op: impl FnMut(&mut T) -> io::Result<R>,
    ) -> io::Result<R> {
        let mut op = op;
        loop {
            match op(self.get_mut()) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return res,
            }
            optimistic(self.readable()).await?;
        }
    }

    pub async fn write_with<R>(&self, op: impl FnMut(&T) -> io::Result<R>) -> io::Result<R> {
        let mut op = op;
        loop {
            match op(self.get_ref()) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return res,
            }
            optimistic(self.writable()).await?;
        }
    }

    pub async fn write_with_mut<R>(
        &mut self,
        op: impl FnMut(&mut T) -> io::Result<R>,
    ) -> io::Result<R> {
        let mut op = op;
        loop {
            match op(self.get_mut()) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return res,
            }
            optimistic(self.writable()).await?;
        }
    }
}

impl<T> Drop for Async<T> {
    fn drop(&mut self) {
        if self.io.is_some() {
            Reactor::get().remove_io(&self.source).ok();

            self.io.take();
        }
    }
}

impl<T: Read> AsyncRead for Async<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            match (&mut *self).get_mut().read(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => {
                    return Poll::Ready(res);
                }
            }
            ready!(self.poll_readable(cx))?;
        }
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        loop {
            match (&mut *self).get_mut().read_vectored(bufs) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }
            ready!(self.poll_readable(cx))?;
        }
    }
}

impl<T> AsyncRead for &Async<T>
where
    for<'a> &'a T: Read,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            match (&*self).get_ref().read(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }

            ready!(self.poll_readable(cx))?;
        }
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        loop {
            match (&*self).get_ref().read_vectored(bufs) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }
            ready!(self.poll_readable(cx))?;
        }
    }
}

impl<T: Write> AsyncWrite for Async<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            match (&mut *self).get_mut().write(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }
            ready!(self.poll_writable(cx))?;
        }
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        loop {
            match (&mut *self).get_mut().write_vectored(bufs) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }
            ready!(self.poll_writable(cx))?;
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            match (&mut *self).get_mut().flush() {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }
            ready!(self.poll_writable(cx))?;
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

impl<T> AsyncWrite for &Async<T>
where
    for<'a> &'a T: Write,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            match (&*self).get_ref().write(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }
            ready!(self.poll_writable(cx))?;
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        loop {
            match (&*self).get_ref().write_vectored(bufs) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }
            ready!(self.poll_writable(cx))?;
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            match (&*self).get_ref().flush() {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }
            ready!(self.poll_writable(cx))?;
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

impl Async<TcpListener> {
    pub async fn bind<A: AsyncToSocketAddrs>(addr: A) -> io::Result<Async<TcpListener>> {
        let addrs = resolve(addr).await?;
        let mut last_err = None;
        for addr in addrs {
            match TcpListener::bind(addr) {
                Ok(listener) => return Ok(Async::new(listener)?),
                Err(e) => last_err = Some(e),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not resolve to any address",
            )
        }))
    }

    pub fn bind_addr<A: Into<SocketAddr>>(addr: A) -> io::Result<Async<TcpListener>> {
        let addr = addr.into();
        Ok(Async::new(TcpListener::bind(addr)?)?)
    }

    pub async fn accept(&self) -> io::Result<(Async<TcpStream>, SocketAddr)> {
        let (stream, addr) = self.read_with(|io| io.accept()).await?;
        Ok((Async::new(stream)?, addr))
    }

    pub fn incoming(&self) -> impl Stream<Item = io::Result<Async<TcpStream>>> + Send + '_ {
        stream::unfold(self, |listener| async move {
            let res = listener.accept().await.map(|(stream, _)| stream);
            Some((res, listener))
        })
    }
}

impl TryFrom<std::net::TcpListener> for Async<std::net::TcpListener> {
    type Error = io::Error;

    fn try_from(listener: std::net::TcpListener) -> io::Result<Self> {
        Async::new(listener)
    }
}

impl Async<TcpStream> {
    pub async fn connect<A: AsyncToSocketAddrs>(addr: A) -> io::Result<Async<TcpStream>> {
        let addrs = resolve(addr).await?;
        let mut last_err = None;
        for addr in addrs {
            match connect::tcp(addr) {
                Ok(fd) => {
                    let stream = Async::new(fd)?;
                    stream.writable().await?;
                    match stream.get_ref().take_error()? {
                        None => return Ok(stream),
                        Some(err) => return Err(err),
                    }
                }
                Err(e) => last_err = Some(e),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not resolve to any address",
            )
        }))
    }

    pub async fn connect_addr<A: Into<SocketAddr>>(addr: A) -> io::Result<Async<TcpStream>> {
        let stream = Async::new(connect::tcp(addr)?)?;

        stream.writable().await?;

        match stream.get_ref().take_error()? {
            None => Ok(stream),
            Some(err) => Err(err),
        }
    }

    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_with(|io| io.peek(buf)).await
    }
}

impl TryFrom<std::net::TcpStream> for Async<std::net::TcpStream> {
    type Error = io::Error;

    fn try_from(stream: std::net::TcpStream) -> io::Result<Self> {
        Async::new(stream)
    }
}

impl Async<UdpSocket> {
    pub fn bind<A: Into<SocketAddr>>(addr: A) -> io::Result<Async<UdpSocket>> {
        let addr = addr.into();
        Ok(Async::new(UdpSocket::bind(addr)?)?)
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.read_with(|io| io.recv_from(buf)).await
    }

    pub async fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.read_with(|io| io.peek_from(buf)).await
    }

    pub async fn send_to<A: Into<SocketAddr>>(&self, buf: &[u8], addr: A) -> io::Result<usize> {
        let addr = addr.into();
        self.write_with(|io| io.send_to(buf, addr)).await
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_with(|io| io.recv(buf)).await
    }

    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_with(|io| io.peek(buf)).await
    }

    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.write_with(|io| io.send(buf)).await
    }
}

impl TryFrom<std::net::UdpSocket> for Async<std::net::UdpSocket> {
    type Error = io::Error;

    fn try_from(socket: std::net::UdpSocket) -> io::Result<Self> {
        Async::new(socket)
    }
}

#[cfg(unix)]
impl Async<UnixListener> {
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Async<UnixListener>> {
        let path = path.as_ref().to_owned();
        Ok(Async::new(UnixListener::bind(path)?)?)
    }

    pub async fn accept(&self) -> io::Result<(Async<UnixStream>, UnixSocketAddr)> {
        let (stream, addr) = self.read_with(|io| io.accept()).await?;
        Ok((Async::new(stream)?, addr))
    }

    pub fn incoming(&self) -> impl Stream<Item = io::Result<Async<UnixStream>>> + Send + '_ {
        stream::unfold(self, |listener| async move {
            let res = listener.accept().await.map(|(stream, _)| stream);
            Some((res, listener))
        })
    }
}

#[cfg(unix)]
impl TryFrom<std::os::unix::net::UnixListener> for Async<std::os::unix::net::UnixListener> {
    type Error = io::Error;

    fn try_from(listener: std::os::unix::net::UnixListener) -> io::Result<Self> {
        Async::new(listener)
    }
}

#[cfg(unix)]
impl Async<UnixStream> {
    pub async fn connect<P: AsRef<Path>>(path: P) -> io::Result<Async<UnixStream>> {
        let stream = Async::new(connect::unix(path)?)?;

        stream.writable().await?;

        stream.get_ref().peer_addr()?;
        Ok(stream)
    }

    pub fn pair() -> io::Result<(Async<UnixStream>, Async<UnixStream>)> {
        let (stream1, stream2) = UnixStream::pair()?;
        Ok((Async::new(stream1)?, Async::new(stream2)?))
    }
}

#[cfg(unix)]
impl TryFrom<std::os::unix::net::UnixStream> for Async<std::os::unix::net::UnixStream> {
    type Error = io::Error;

    fn try_from(stream: std::os::unix::net::UnixStream) -> io::Result<Self> {
        Async::new(stream)
    }
}

#[cfg(unix)]
impl Async<UnixDatagram> {
    pub fn bind<P: AsRef<Path>>(path: P) -> io::Result<Async<UnixDatagram>> {
        let path = path.as_ref().to_owned();
        Ok(Async::new(UnixDatagram::bind(path)?)?)
    }

    pub fn unbound() -> io::Result<Async<UnixDatagram>> {
        Ok(Async::new(UnixDatagram::unbound()?)?)
    }

    pub fn pair() -> io::Result<(Async<UnixDatagram>, Async<UnixDatagram>)> {
        let (socket1, socket2) = UnixDatagram::pair()?;
        Ok((Async::new(socket1)?, Async::new(socket2)?))
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, UnixSocketAddr)> {
        self.read_with(|io| io.recv_from(buf)).await
    }

    pub async fn send_to<P: AsRef<Path>>(&self, buf: &[u8], path: P) -> io::Result<usize> {
        self.write_with(|io| io.send_to(buf, &path)).await
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_with(|io| io.recv(buf)).await
    }

    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.write_with(|io| io.send(buf)).await
    }
}

#[cfg(unix)]
impl TryFrom<std::os::unix::net::UnixDatagram> for Async<std::os::unix::net::UnixDatagram> {
    type Error = io::Error;

    fn try_from(socket: std::os::unix::net::UnixDatagram) -> io::Result<Self> {
        Async::new(socket)
    }
}

async fn optimistic(fut: impl Future<Output = io::Result<()>>) -> io::Result<()> {
    let mut polled = false;
    pin!(fut);

    future::poll_fn(|cx| {
        if !polled {
            polled = true;
            fut.as_mut().poll(cx)
        } else {
            Poll::Ready(Ok(()))
        }
    })
    .await
}
