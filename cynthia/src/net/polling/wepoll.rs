use std::convert::TryInto;
use std::os::windows::io::RawSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::{io, ptr};
use wepoll_sys as we;
use winapi::ctypes;

use crate::Event;

const READ_FLAGS: u32 = we::EPOLLIN | we::EPOLLRDHUP | we::EPOLLHUP | we::EPOLLERR | we::EPOLLPRI;
const WRITE_FLAGS: u32 = we::EPOLLOUT | we::EPOLLHUP | we::EPOLLERR;

pub struct Events {
    list: Box<[we::epoll_event]>,
    len: usize,
}

unsafe impl Send for Events {}

impl Events {
    pub fn new() -> Events {
        let ev = we::epoll_event {
            events: 0,
            data: we::epoll_data { u64: 0 },
        };
        Events {
            list: vec![ev; 1000].into_boxed_slice(),
            len: 0,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = Event> + '_ {
        self.list[..self.len].iter().map(|ev| Event {
            key: unsafe { ev.data.u64 } as usize,
            readable: (ev.events & READ_FLAGS) != 0,
            writable: (ev.events & WRITE_FLAGS) != 0,
        })
    }
}

macro_rules! wepoll {
    ($fn:ident $args:tt) => {{
        let res = unsafe { we::$fn $args };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }};
}

#[derive(Debug)]
pub struct Poller {
    handle: we::HANDLE,
    notified: AtomicBool,
}

unsafe impl Send for Poller {}
unsafe impl Sync for Poller {}

impl Poller {
    pub fn new() -> io::Result<Poller> {
        let handle = unsafe { we::epoll_create1(0) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let notified = AtomicBool::new(false);
        Ok(Poller { handle, notified })
    }

    pub fn add(&self, sock: RawSocket, ev: Event) -> io::Result<()> {
        self.ctl(we::EPOLL_CTL_ADD, sock, Some(ev))
    }

    pub fn modify(&self, sock: RawSocket, ev: Event) -> io::Result<()> {
        self.ctl(we::EPOLL_CTL_MOD, sock, Some(ev))
    }

    pub fn delete(&self, sock: RawSocket) -> io::Result<()> {
        self.ctl(we::EPOLL_CTL_DEL, sock, None)
    }

    pub fn wait(&self, events: &mut Events, timeout: Option<Duration>) -> io::Result<()> {
        let deadline = timeout.map(|t| Instant::now() + t);

        loop {
            let timeout_ms = match deadline.map(|d| d.saturating_duration_since(Instant::now())) {
                None => -1,
                Some(t) => {
                    let mut ms = t.as_millis().try_into().unwrap_or(std::u64::MAX);
                    if Duration::from_millis(ms) < t {
                        ms += 1;
                    }
                    ms.try_into().unwrap_or(std::i32::MAX)
                }
            };

            events.len = wepoll!(epoll_wait(
                self.handle,
                events.list.as_mut_ptr(),
                events.list.len() as ctypes::c_int,
                timeout_ms,
            ))? as usize;

            if self.notified.swap(false, Ordering::SeqCst) || events.len > 0 || timeout_ms == 0 {
                break;
            }
        }

        Ok(())
    }

    pub fn notify(&self) -> io::Result<()> {
        if self
            .notified
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            unsafe {
                winapi::um::ioapiset::PostQueuedCompletionStatus(
                    self.handle as winapi::um::winnt::HANDLE,
                    0,
                    0,
                    ptr::null_mut(),
                );
            }
        }
        Ok(())
    }

    fn ctl(&self, op: u32, sock: RawSocket, ev: Option<Event>) -> io::Result<()> {
        let mut ev = ev.map(|ev| {
            let mut flags = we::EPOLLONESHOT;
            if ev.readable {
                flags |= READ_FLAGS;
            }
            if ev.writable {
                flags |= WRITE_FLAGS;
            }
            we::epoll_event {
                events: flags as u32,
                data: we::epoll_data { u64: ev.key as u64 },
            }
        });
        wepoll!(epoll_ctl(
            self.handle,
            op as ctypes::c_int,
            sock as we::SOCKET,
            ev.as_mut()
                .map(|ev| ev as *mut we::epoll_event)
                .unwrap_or(ptr::null_mut()),
        ))?;
        Ok(())
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        unsafe {
            we::epoll_close(self.handle);
        }
    }
}
