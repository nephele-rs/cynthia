#![forbid(unsafe_code)]

#[cfg(unix)]
pub mod unix;

mod addr;
mod tcp;
mod udp;

pub use addr::AsyncToSocketAddrs;
pub use tcp::{Incoming, TcpListener, TcpStream};
pub use udp::UdpSocket;

use std::io;

pub use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, 
    Shutdown, 
    SocketAddr, SocketAddrV4, SocketAddrV6
};

pub use std::net::AddrParseError;

pub async fn resolve<A: AsyncToSocketAddrs>(addr: A) -> io::Result<Vec<SocketAddr>> {
    Ok(addr.to_socket_addrs().await?.collect())
}
