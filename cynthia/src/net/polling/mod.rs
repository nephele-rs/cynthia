#![cfg(feature = "std")]
#![cfg_attr(not(feature = "std"), no_std)]

use cfg_if::cfg_if;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use std::usize;
use std::{fmt, io};

#[cfg(unix)]
macro_rules! syscall {
    ($fn:ident $args:tt) => {{
        let res = unsafe { libc::$fn $args };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }};
}

cfg_if! {
    if #[cfg(any(target_os = "linux", target_os = "android"))] {
        mod epoll;
        use epoll as sys;
    } else if #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly",
    ))] {
        mod kqueue;
        use kqueue as sys;
    } else if #[cfg(target_os = "windows")] {
        mod wepoll;
        use wepoll as sys;
    } else {
        compile_error!("polling does not support this target OS");
    }
}

const NOTIFY_KEY: usize = std::usize::MAX;

#[derive(Debug)]
pub struct Event {
    pub key: usize,
    pub readable: bool,
    pub writable: bool,
}

impl Event {
    pub fn all(key: usize) -> Event {
        Event {
            key,
            readable: true,
            writable: true,
        }
    }

    pub fn readable(key: usize) -> Event {
        Event {
            key,
            readable: true,
            writable: false,
        }
    }

    pub fn writable(key: usize) -> Event {
        Event {
            key,
            readable: false,
            writable: true,
        }
    }

    pub fn none(key: usize) -> Event {
        Event {
            key,
            readable: false,
            writable: false,
        }
    }
}

pub struct Poller {
    poller: sys::Poller,
    events: Mutex<sys::Events>,
    notified: AtomicBool,
}

impl Poller {
    pub fn new() -> io::Result<Poller> {
        Ok(Poller {
            poller: sys::Poller::new()?,
            events: Mutex::new(sys::Events::new()),
            notified: AtomicBool::new(false),
        })
    }

    pub fn add(&self, source: impl Source, interest: Event) -> io::Result<()> {
        if interest.key == NOTIFY_KEY {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the key is not allowed to be `usize::MAX`",
            ));
        }
        self.poller.add(source.raw(), interest)
    }

    pub fn modify(&self, source: impl Source, interest: Event) -> io::Result<()> {
        if interest.key == NOTIFY_KEY {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the key is not allowed to be `usize::MAX`",
            ));
        }
        self.poller.modify(source.raw(), interest)
    }

    pub fn delete(&self, source: impl Source) -> io::Result<()> {
        self.poller.delete(source.raw())
    }

    pub fn wait(&self, events: &mut Vec<Event>, timeout: Option<Duration>) -> io::Result<usize> {
        if let Ok(mut lock) = self.events.try_lock() {
            self.poller.wait(&mut lock, timeout)?;

            self.notified.swap(false, Ordering::SeqCst);

            let len = events.len();
            events.extend(lock.iter().filter(|ev| ev.key != usize::MAX));
            Ok(events.len() - len)
        } else {
            Ok(0)
        }
    }

    pub fn notify(&self) -> io::Result<()> {
        if self
            .notified
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.poller.notify()?;
        }
        Ok(())
    }
}

impl fmt::Debug for Poller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.poller.fmt(f)
    }
}

cfg_if! {
    if #[cfg(unix)] {
        use std::os::unix::io::{AsRawFd, RawFd};

        pub trait Source {
            fn raw(&self) -> RawFd;
        }

        impl Source for RawFd {
            fn raw(&self) -> RawFd {
                *self
            }
        }

        impl<T: AsRawFd> Source for &T {
            fn raw(&self) -> RawFd {
                self.as_raw_fd()
            }
        }
    } else if #[cfg(windows)] {
        use std::os::windows::io::{AsRawSocket, RawSocket};

        pub trait Source {
            fn raw(&self) -> RawSocket;
        }

        impl Source for RawSocket {
            fn raw(&self) -> RawSocket {
                *self
            }
        }

        impl<T: AsRawSocket> Source for &T {
            fn raw(&self) -> RawSocket {
                self.as_raw_socket()
            }
        }
    }
}
