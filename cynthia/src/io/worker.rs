use once_cell::sync::Lazy;
use std::cell::Cell;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::io::reactor::Reactor;
use crate::pin;
use crate::platform::parking;
use crate::platform::waker::waker_fn;

static BLOCK_ON_GLOBAL: AtomicUsize = AtomicUsize::new(0);

struct CallOnDrop<F: Fn()>(F);

impl<F: Fn()> Drop for CallOnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

static WAKER: Lazy<parking::Unparker> = Lazy::new(|| {
    let (parker, unparker) = parking::pair();
    thread::Builder::new()
        .name("cynthia-io".to_string())
        .spawn(move || cycle_entry(parker))
        .expect("cannot spawn cynthia-io thread");

    unparker
});

#[inline]
fn wake() {
    WAKER.unpark();
}

pub(crate) fn init() {
    Lazy::force(&WAKER);
}

fn cycle_entry(parker: parking::Parker) {
    let mut last_tick = 0;
    let mut sleeps = 0u64;

    loop {
        let tick = Reactor::get().ticker();

        if last_tick == tick {
            let locked_reactor = if sleeps >= 10 {
                Some(Reactor::get().lock())
            } else {
                Reactor::get().try_lock()
            };

            if let Some(mut locked_reactor) = locked_reactor {
                locked_reactor.react(None).ok();
                last_tick = Reactor::get().ticker();
                sleeps = 0;
            }
        } else {
            last_tick = tick;
        }

        if BLOCK_ON_GLOBAL.load(Ordering::SeqCst) > 0 {
            let delay_us = [50, 75, 100, 250, 500, 750, 1000, 2500, 5000]
                .get(sleeps as usize)
                .unwrap_or(&10_000);

            if parker.park_timeout(Duration::from_micros(*delay_us)) {
                last_tick = Reactor::get().ticker();
                sleeps = 0;
            } else {
                sleeps += 1;
            }
        }
    }
}

pub fn block_on<T>(future: impl Future<Output = T>) -> T {
    use std::sync::Arc;
    use std::task::{Context, Poll};

    BLOCK_ON_GLOBAL.fetch_add(1, Ordering::SeqCst);

    let _guard = CallOnDrop(|| {
        BLOCK_ON_GLOBAL.fetch_sub(1, Ordering::SeqCst);
        wake();
    });

    let (p, u) = parking::pair();
    let block_stub = Arc::new(AtomicBool::new(false));

    thread_local! {
        static CACHED_IO: Cell<bool> = Cell::new(false);
    }

    let waker = waker_fn({
        let block_stub = block_stub.clone();
        move || {
            if u.unpark() {
                if !CACHED_IO.with(Cell::get) && block_stub.load(Ordering::SeqCst) {
                    Reactor::get().notify();
                }
            }
        }
    });
    let cx = &mut Context::from_waker(&waker);
    pin!(future);

    loop {
        if let Poll::Ready(t) = future.as_mut().poll(cx) {
            return t;
        }

        if p.park_timeout(Duration::from_secs(0)) {
            if let Some(mut locked_reactor) = Reactor::get().try_lock() {
                CACHED_IO.with(|io| io.set(true));
                let _guard = CallOnDrop(|| {
                    CACHED_IO.with(|io| io.set(false));
                });

                locked_reactor.react(Some(Duration::from_secs(0))).ok();
            }
            continue;
        }

        if let Some(mut locked_reactor) = Reactor::get().try_lock() {
            let start = Instant::now();
            loop {
                CACHED_IO.with(|io| io.set(true));
                block_stub.store(true, Ordering::SeqCst);
                let _guard = CallOnDrop(|| {
                    CACHED_IO.with(|io| io.set(false));
                    block_stub.store(false, Ordering::SeqCst);
                });

                if p.park_timeout(Duration::from_secs(0)) {
                    break;
                }

                locked_reactor.react(None).ok();

                if p.park_timeout(Duration::from_secs(0)) {
                    break;
                }

                if start.elapsed() > Duration::from_micros(500) {
                    drop(locked_reactor);

                    wake();

                    p.park();
                    break;
                }
            }
        } else {
            p.park();
        }
    }
}
