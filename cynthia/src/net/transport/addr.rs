use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, ToSocketAddrs};

use std::pin::Pin;
use std::task::{Context, Poll};
use std::{fmt, io, mem};

use crate::future::future;
use crate::runtime::blocking::unblock;

pub trait AsyncToSocketAddrs: Sealed {}

pub trait Sealed {
    type Iter: Iterator<Item = SocketAddr> + Unpin;
    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter>;
}

pub enum SocketAddrFuture<I> {
    Inflight(future::Boxed<io::Result<I>>),
    Ready(io::Result<I>),
    Finish,
}

impl<I> fmt::Debug for SocketAddrFuture<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SocketAddrFuture")
    }
}

impl<I: Iterator<Item = SocketAddr> + Unpin> Future for SocketAddrFuture<I> {
    type Output = io::Result<I>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let state = mem::replace(&mut *self, SocketAddrFuture::Finish);

        match state {
            SocketAddrFuture::Inflight(mut task) => {
                let poll = Pin::new(&mut task).poll(cx);
                if poll.is_pending() {
                    *self = SocketAddrFuture::Inflight(task);
                }
                poll
            }
            SocketAddrFuture::Ready(res) => Poll::Ready(res),
            SocketAddrFuture::Finish => panic!("polled a completed future"),
        }
    }
}

impl AsyncToSocketAddrs for SocketAddr {}

impl Sealed for SocketAddr {
    type Iter = std::option::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        SocketAddrFuture::Ready(Ok(Some(*self).into_iter()))
    }
}

impl AsyncToSocketAddrs for SocketAddrV4 {}

impl Sealed for SocketAddrV4 {
    type Iter = std::option::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        Sealed::to_socket_addrs(&SocketAddr::V4(*self))
    }
}

impl AsyncToSocketAddrs for SocketAddrV6 {}

impl Sealed for SocketAddrV6 {
    type Iter = std::option::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        Sealed::to_socket_addrs(&SocketAddr::V6(*self))
    }
}

impl AsyncToSocketAddrs for (IpAddr, u16) {}

impl Sealed for (IpAddr, u16) {
    type Iter = std::option::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        let (ip, port) = *self;
        match ip {
            IpAddr::V4(a) => Sealed::to_socket_addrs(&(a, port)),
            IpAddr::V6(a) => Sealed::to_socket_addrs(&(a, port)),
        }
    }
}

impl AsyncToSocketAddrs for (Ipv4Addr, u16) {}

impl Sealed for (Ipv4Addr, u16) {
    type Iter = std::option::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        let (ip, port) = *self;
        Sealed::to_socket_addrs(&SocketAddrV4::new(ip, port))
    }
}

impl AsyncToSocketAddrs for (Ipv6Addr, u16) {}

impl Sealed for (Ipv6Addr, u16) {
    type Iter = std::option::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        let (ip, port) = *self;
        Sealed::to_socket_addrs(&SocketAddrV6::new(ip, port, 0, 0))
    }
}

impl AsyncToSocketAddrs for (&str, u16) {}

impl Sealed for (&str, u16) {
    type Iter = std::vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        let (host, port) = *self;

        if let Ok(addr) = host.parse::<Ipv4Addr>() {
            let addr = SocketAddrV4::new(addr, port);
            return SocketAddrFuture::Ready(Ok(vec![SocketAddr::V4(addr)].into_iter()));
        }

        if let Ok(addr) = host.parse::<Ipv6Addr>() {
            let addr = SocketAddrV6::new(addr, port, 0, 0);
            return SocketAddrFuture::Ready(Ok(vec![SocketAddr::V6(addr)].into_iter()));
        }

        let host = host.to_string();
        let future = unblock(move || {
            let addr = (host.as_str(), port);
            ToSocketAddrs::to_socket_addrs(&addr)
        });
        SocketAddrFuture::Inflight(Box::pin(future))
    }
}

impl AsyncToSocketAddrs for (String, u16) {}

impl Sealed for (String, u16) {
    type Iter = std::vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        Sealed::to_socket_addrs(&(&*self.0, self.1))
    }
}

impl AsyncToSocketAddrs for str {}

impl Sealed for str {
    type Iter = std::vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        if let Ok(addr) = self.parse() {
            return SocketAddrFuture::Ready(Ok(vec![addr].into_iter()));
        }

        let addr = self.to_string();
        let future = unblock(move || std::net::ToSocketAddrs::to_socket_addrs(addr.as_str()));
        SocketAddrFuture::Inflight(Box::pin(future))
    }
}

impl AsyncToSocketAddrs for &[SocketAddr] {}

impl<'a> Sealed for &'a [SocketAddr] {
    type Iter = std::iter::Cloned<std::slice::Iter<'a, SocketAddr>>;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        SocketAddrFuture::Ready(Ok(self.iter().cloned()))
    }
}

impl<T: AsyncToSocketAddrs + ?Sized> AsyncToSocketAddrs for &T {}

impl<T: Sealed + ?Sized> Sealed for &T {
    type Iter = T::Iter;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        Sealed::to_socket_addrs(&**self)
    }
}

impl AsyncToSocketAddrs for String {}

impl Sealed for String {
    type Iter = std::vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> SocketAddrFuture<Self::Iter> {
        Sealed::to_socket_addrs(&**self)
    }
}
