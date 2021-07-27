use anyhow::{Context as _, Result};
use std::net::{ToSocketAddrs};
use cynthia::runtime::transport::resolve;

#[cynthia::main]
async fn main() -> Result<()> {
    let host1 = "www.rust-lang.org".to_string();
    let port: u16 = 8080;

    let addr1 = {
        let host = host1.clone();
        cynthia::runtime::unblock(move || (host, port).to_socket_addrs())
            .await?
            .next()
            .context("cannot resolve address")?
    };

    println!("socket_addr = {}", addr1);

    let host2 = "www.baidu.com:80";
    let addr2 = resolve(host2).await?;
    println!("socket_addr = {:?}", addr2);

    Ok(())
}