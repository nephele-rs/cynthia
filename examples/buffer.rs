use cynthia::runtime::BytePool;
use std::sync::Arc;

lazy_static::lazy_static! {
    static ref POOL: Arc<BytePool> = Arc::new(BytePool::new());
}

fn main() {
    let mut buffer = POOL.alloc(1024);
    for i in 0..1024 {
        buffer[i] = (i % 255) as u8;
    }

    assert_eq!(buffer[1], 1);
    assert_eq!(buffer[55], 55);
    assert_eq!(buffer[1023], (1023 % 255) as u8);

    drop(buffer);
}
