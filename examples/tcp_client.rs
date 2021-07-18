use std::net::TcpStream;

use cynthia::runtime::{self, future, swap, Async, Unblock};

fn main() -> swap::Result<()> {
    runtime::block_on(async {
        let stdin = Unblock::new(std::io::stdin());
        let mut stdout = Unblock::new(std::io::stdout());

        let stream = Async::<TcpStream>::connect(([127, 0, 0, 1], 7000)).await?;

        future::try_zip(
            swap::copy(stdin, &mut &stream),
            swap::copy(&stream, &mut stdout),
        )
        .await?;

        Ok(())
    })
}
