# cynthia
A simple Rust asynchronous runtime.

# Overview
cynthia is a simple, event-driven, non-blocking I/O platform for writing asynchronous applications with the Rust programming language. At a high level, it provides a few major components:
* A multithreaded, work-stealing based task scheduler.
* A reactor backed by the operating system's event queue, support epoll, kqueue, IOCP, etc...
* Asynchronous TCP/UDP/Unix domain sockets.
* Some necessary components for asynchronous programming, such as channel, message, etc.

# Example
A basic TCP echo server with cynthia.

Make sure you activated the full features of the cynthia crate on Cargo.toml:
```
[dependencies]
cynthia = { version = "0.0.6", features = ["full"]}
```

Then, on your main.rs:
```
use std::net::{TcpListener, TcpStream};

use cynthia::future::prelude::*;
use cynthia::runtime::{self, swap, Async};

async fn echo(mut stream: Async<TcpStream>) -> swap::Result<()> {
    let mut buf = vec![0u8; 1024];

    loop {
        let n = stream.read(&mut buf).await?;

        if n == 0 {
            return Ok(());
        }

        stream.write_all(&buf[0..n]).await?;
    }
}

#[cynthia::main]
async fn main() -> swap::Result<()> {
    let listener = Async::<TcpListener>::bind("127.0.0.1:7000").await?;

    loop {
        let (stream, _peer_addr) = listener.accept().await?;
        runtime::spawn(echo(stream)).detach();
    }
}
```

More examples can be found [here](https://github.com/nephele-rs/cynthia/tree/main/examples)

# Supported Rust Versions
This library is verified to work in rustc 1.51.0 (nightly), and the support of other versions needs more testing.

# License
This project is licensed under the [Apache License 2.0](https://github.com/nephele-rs/cynthia/blob/main/LICENSE).
