use std::convert::TryFrom;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, RawSocket};
use std::sync::Arc;

use crate::io::Async;
use crate::net::transport::addr::AsyncToSocketAddrs;

#[derive(Clone, Debug)]
pub struct UdpSocket {
    inner: Arc<Async<std::net::UdpSocket>>,
}

impl UdpSocket {
    fn new(inner: Arc<Async<std::net::UdpSocket>>) -> UdpSocket {
        UdpSocket { inner }
    }

    pub async fn bind<A: AsyncToSocketAddrs>(addr: A) -> io::Result<UdpSocket> {
        let mut last_err = None;

        for addr in addr.to_socket_addrs().await? {
            match Async::<std::net::UdpSocket>::bind(addr) {
                Ok(socket) => return Ok(UdpSocket::new(Arc::new(socket))),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not bind to any of the addresses",
            )
        }))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.get_ref().peer_addr()
    }

    pub async fn connect<A: AsyncToSocketAddrs>(&self, addr: A) -> io::Result<()> {
        let mut last_err = None;

        for addr in addr.to_socket_addrs().await? {
            match self.inner.get_ref().connect(addr) {
                Ok(()) => return Ok(()),
                Err(err) => last_err = Some(err),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not connect to any of the addresses",
            )
        }))
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.recv_from(buf).await
    }

    pub async fn peek_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.inner.get_ref().peek_from(buf)
    }

    pub async fn send_to<A: AsyncToSocketAddrs>(&self, buf: &[u8], addr: A) -> io::Result<usize> {
        let addr = match addr.to_socket_addrs().await?.next() {
            Some(addr) => addr,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "no addresses to send data to",
                ))
            }
        };

        self.inner.send_to(buf, addr).await
    }

    pub async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.recv(buf).await
    }

    pub async fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.peek(buf).await
    }

    pub async fn send(&self, buf: &[u8]) -> io::Result<usize> {
        self.inner.send(buf).await
    }

    pub fn broadcast(&self) -> io::Result<bool> {
        self.inner.get_ref().broadcast()
    }

    pub fn set_broadcast(&self, broadcast: bool) -> io::Result<()> {
        self.inner.get_ref().set_broadcast(broadcast)
    }

    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        self.inner.get_ref().multicast_loop_v4()
    }

    pub fn set_multicast_loop_v4(&self, multicast_loop_v4: bool) -> io::Result<()> {
        self.inner
            .get_ref()
            .set_multicast_loop_v4(multicast_loop_v4)
    }

    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        self.inner.get_ref().multicast_ttl_v4()
    }

    pub fn set_multicast_ttl_v4(&self, ttl: u32) -> io::Result<()> {
        self.inner.get_ref().set_multicast_ttl_v4(ttl)
    }

    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        self.inner.get_ref().multicast_loop_v6()
    }

    pub fn set_multicast_loop_v6(&self, multicast_loop_v6: bool) -> io::Result<()> {
        self.inner
            .get_ref()
            .set_multicast_loop_v6(multicast_loop_v6)
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

    pub fn join_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        self.inner
            .get_ref()
            .join_multicast_v4(&multiaddr, &interface)
    }

    pub fn leave_multicast_v4(&self, multiaddr: Ipv4Addr, interface: Ipv4Addr) -> io::Result<()> {
        self.inner
            .get_ref()
            .leave_multicast_v4(&multiaddr, &interface)
    }

    pub fn join_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()> {
        self.inner.get_ref().join_multicast_v6(multiaddr, interface)
    }

    pub fn leave_multicast_v6(&self, multiaddr: &Ipv6Addr, interface: u32) -> io::Result<()> {
        self.inner
            .get_ref()
            .leave_multicast_v6(multiaddr, interface)
    }
}

impl From<Async<std::net::UdpSocket>> for UdpSocket {
    fn from(socket: Async<std::net::UdpSocket>) -> UdpSocket {
        UdpSocket::new(Arc::new(socket))
    }
}

impl TryFrom<std::net::UdpSocket> for UdpSocket {
    type Error = io::Error;

    fn try_from(socket: std::net::UdpSocket) -> io::Result<UdpSocket> {
        Ok(UdpSocket::new(Arc::new(Async::new(socket)?)))
    }
}

impl Into<Arc<Async<std::net::UdpSocket>>> for UdpSocket {
    fn into(self) -> Arc<Async<std::net::UdpSocket>> {
        self.inner
    }
}

#[cfg(unix)]
impl AsRawFd for UdpSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for UdpSocket {
    fn as_raw_socket(&self) -> RawSocket {
        self.inner.as_raw_socket()
    }
}
