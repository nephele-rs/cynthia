use pin_project_lite::pin_project;
use std::future::Future;
pub use std::io::{Error, Result};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Waker;

use crate::future::future;
use crate::runtime::task::{Context, Poll};

pin_project! {
    #[derive(Clone)]
    pub struct WaitGroup {
        #[pin]
        inner: Arc<WaitGroupInner>,
        waker: Option<Waker>,
        source: bool,
    }
}

struct WaitGroupInner {
    pub count: Mutex<i32>,
}

impl WaitGroup {
    pub fn new() -> WaitGroup {
        WaitGroup {
            inner: Arc::new(WaitGroupInner {
                count: Mutex::new(0),
            }),
            waker: None,
            source: true,
        }
    }

    pub async fn wait_clone(&self) -> Result<WaitGroup> {
        let i = self.inner.clone();
        let mut wg = WaitGroup {
            inner: i,
            waker: None,
            source: false,
        };

        wg.attach().await?;

        Ok(wg)
    }

    fn register(&mut self, new_waker: &Waker) {
        self.waker = Some(new_waker.clone());
    }

    async fn attach(&mut self) -> Result<()> {
        future::poll_fn(|cx| {
            self.register(cx.waker());
            Poll::Ready(Ok::<(), Error>(()))
        })
        .await?;

        let mut count = self.inner.count.lock().unwrap();
        *count += 1;

        Ok(())
    }

    fn done(&mut self) -> Result<()> {
        let mut count = self.inner.count.lock().unwrap();
        *count -= 1;
        if *count == 0 {
            if let Some(w) = self.waker.take() {
                return Ok(w.wake());
            }
        }

        Ok(())
    }
}

impl Future for WaitGroup {
    type Output = Result<()>;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.source {
            let count = self.inner.count.lock().unwrap();
            if *count > 0 {
                return Poll::Pending;
            } else {
                return Poll::Ready(Ok(()));
            }
        } else {
            match self.done() {
                Ok(()) => {
                    return Poll::Ready(Ok(()));
                }
                Err(_e) => {
                    return Poll::Pending;
                }
            }
        }
    }
}
