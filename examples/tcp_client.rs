use std::net::TcpStream;

use cynthia::runtime::{future, swap, Async, Unblock};

#[cynthia::main]
async fn main() -> swap::Result<()> {
    let stdin = Unblock::new(std::io::stdin());
    let mut stdout = Unblock::new(std::io::stdout());

    let stream = Async::<TcpStream>::connect(([127, 0, 0, 1], 7000)).await?;

    future::try_zip(
        swap::copy(stdin, &mut &stream),
        swap::copy(&stream, &mut stdout),
    )
    .await?;

    Ok(())
}
