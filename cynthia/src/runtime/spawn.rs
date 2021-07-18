use once_cell::sync::Lazy;
use std::future::Future;
use std::panic::catch_unwind;
use std::thread;

use crate::future::future;
use crate::io::block_on;
use crate::runtime::executor::Executor;
use crate::runtime::task::Task;

pub fn spawn<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> Task<T> {
    static GLOBAL: Lazy<Executor<'_>> = Lazy::new(|| {
        let num_threads = {
            std::env::var("CYNTHIA_THREADS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1)
        };

        for n in 1..=num_threads {
            thread::Builder::new()
                .name(format!("cyn-{}", n))
                .spawn(|| loop {
                    catch_unwind(|| block_on(GLOBAL.run(future::pending::<()>()))).ok();
                })
                .expect("spawn executor thread failed");
        }

        Executor::new()
    });

    GLOBAL.spawn(future)
}
