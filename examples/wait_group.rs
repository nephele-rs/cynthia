use anyhow::Result;
use std::io::Error;
use std::net::{TcpListener, TcpStream};

use cynthia::runtime::WaitGroup;
use cynthia::runtime::{self, swap, Async};

async fn process(i: i32) -> Result<()> {
    println!("hello process {}", i);
    Ok(())
}
async fn handle(stream: Async<TcpStream>) -> Result<()> {
    let wg = WaitGroup::new();

    let mut i: i32 = 0;
    while i < 20 {
        let wg = wg.wait_clone().await?;

        runtime::spawn(async move {
            process(i).await.map_err(|err| println!("{:?}", err)).ok();
            wg.await?;
            Ok::<(), Error>(())
        })
        .detach();

        i += 1;
    }

    println!("before, wg wait");
    wg.await?;
    println!("after, wg wait");

    swap::copy(&stream, &mut &stream).await?;
    Ok(())
}

#[cynthia::main]
async fn main() -> Result<()> {
    let listener = Async::<TcpListener>::bind(([127, 0, 0, 1], 7000))?;
    loop {
        let (stream, _peer_addr) = listener.accept().await?;
        runtime::spawn(handle(stream)).detach();
    }
}
