use std::net::{TcpStream, TcpListener};
use cynthia::runtime::{self, swap, Async};

async fn echo(stream: Async<TcpStream>) -> swap::Result<()> {
    swap::copy(&stream, &mut &stream).await?;
    Ok(())
}

#[cynthia::main]
async fn main() -> swap::Result<()> {
    let listener = Async::<TcpListener>::bind(([127, 0, 0, 1], 7000))?;
    loop {
        let (stream, _peer_addr) = listener.accept().await?;
        runtime::spawn(echo(stream)).detach();
    }
}
