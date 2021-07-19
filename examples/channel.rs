use anyhow::Result;
use std::net::{TcpListener, TcpStream};

use cynthia::future::StreamExt;
use cynthia::runtime::channel;
use cynthia::runtime::{self, swap, Async};

async fn handle(stream: Async<TcpStream>) -> Result<()> {
    let channel_count = 20;
    let (s, mut r) = channel::unbounded();

    let mut i: i32 = 0;
    while i < channel_count {
        let s1 = s.clone();
        runtime::spawn(async move {
            let data = format!("hello, task {}", i);
            s1.send(data).await?;
            Ok::<(), channel::SendError<String>>(())
        })
        .detach();

        i += 1;
    }

    println!("before, channel wait");

    let mut j: i32 = 0;
    while let Some(item) = r.next().await {
        println!("after, channel wait {:?}", item);
        j += 1;

        if j == channel_count {
            break;
        }
    }

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
