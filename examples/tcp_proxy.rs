use std::net::{TcpListener, TcpStream};

use cynthia::runtime::{self, future, swap, Async};

async fn proxy(inbound: Async<TcpStream>) -> swap::Result<()> {
    let outbound = Async::<TcpStream>::connect(([127, 0, 0, 1], 7000)).await?;

    let downstream = async { swap::copy(&inbound, &outbound).await };

    let upstream = async { swap::copy(&outbound, &inbound).await };

    future::try_zip(downstream, upstream).await?;

    Ok(())
}

fn get_string() -> swap::Result<String> {
    let mut buffer = String::new();

    std::io::stdin().read_line(&mut buffer)?;

    Ok(buffer)
}

fn main() -> swap::Result<()> {
    runtime::block_on(async {
        let listener = Async::<TcpListener>::bind(([127, 0, 0, 1], 8000))?;
        get_string().unwrap();
        loop {
            let (stream, _peer_addr) = listener.accept().await?;
            runtime::spawn(proxy(stream)).detach();
        }
    })
}
