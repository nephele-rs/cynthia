use std::time::Duration;

use cynthia::runtime::connect::connect;
use cynthia::runtime::polling::{Event, Poller};
use cynthia::runtime::swap;
use cynthia::runtime::transport::TcpStream;

#[cynthia::main]
async fn main() -> swap::Result<()> {
    let _stream = TcpStream::connect("127.0.0.1:7000").await?;
    std::io::Result::Ok(())
}

#[allow(dead_code)]
fn main1() -> swap::Result<()> {
    let stream = connect::tcp(([127, 0, 0, 1], 7000))?;
    let poller = Poller::new()?;
    poller.add(&stream, Event::writable(0))?;

    if poller.wait(&mut Vec::new(), Some(Duration::from_secs(1)))? == 0 {
        println!("timeout");
    } else if let Some(err) = stream.take_error()? {
        println!("error: {}", err);
    } else {
        println!("connected");
    }
    std::io::Result::Ok(())
}
