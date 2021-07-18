use std::net::{TcpListener, TcpStream};

use cynthia::runtime::{self, swap, Async};

async fn echo(stream: Async<TcpStream>) -> swap::Result<()> {
    swap::copy(&stream, &mut &stream).await?;
    Ok(())
}

fn main() -> swap::Result<()> {
    runtime::block_on(async {
        let listener = Async::<TcpListener>::bind(([127, 0, 0, 1], 7000))?;

        loop {
            let (stream, _peer_addr) = listener.accept().await?;
            runtime::spawn(echo(stream)).detach();
        }
    })
}
