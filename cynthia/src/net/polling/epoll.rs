use std::convert::TryInto;
use std::os::unix::io::RawFd;
use std::time::Duration;
use std::{io, ptr};

use crate::Event;

pub struct Events {
    list: Box<[libc::epoll_event]>,
    len: usize,
}

unsafe impl Send for Events {}

impl Events {
    pub fn new() -> Events {
        let ev = libc::epoll_event { events: 0, u64: 0 };
        let list = vec![ev; 1000].into_boxed_slice();
        let len = 0;
        Events { list, len }
    }

    pub fn iter(&self) -> impl Iterator<Item = Event> + '_ {
        self.list[..self.len].iter().map(|ev| Event {
            key: ev.u64 as usize,
            readable: (ev.events as libc::c_int & read_flags()) != 0,
            writable: (ev.events as libc::c_int & write_flags()) != 0,
        })
    }
}

#[derive(Debug)]
pub struct Poller {
    epoll_fd: RawFd,
    event_fd: RawFd,
    timer_fd: Option<RawFd>,
}

impl Poller {
    pub fn new() -> io::Result<Poller> {
        let epoll_fd = syscall!(syscall(
            libc::SYS_epoll_create1,
            libc::EPOLL_CLOEXEC as libc::c_int
        ))
        .map(|fd| fd as libc::c_int)
        .or_else(|e| match e.raw_os_error() {
            Some(libc::ENOSYS) => {
                let fd = syscall!(epoll_create(1024))?;

                if let Ok(flags) = syscall!(fcntl(fd, libc::F_GETFD)) {
                    let _ = syscall!(fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC));
                }

                Ok(fd)
            }
            _ => Err(e),
        })?;

        let event_fd = syscall!(eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK))?;
        let timer_fd = syscall!(syscall(
            libc::SYS_timerfd_create,
            libc::CLOCK_MONOTONIC as libc::c_int,
            (libc::TFD_CLOEXEC | libc::TFD_NONBLOCK) as libc::c_int,
        ))
        .map(|fd| fd as libc::c_int)
        .ok();

        let poller = Poller {
            epoll_fd,
            event_fd,
            timer_fd,
        };

        if let Some(timer_fd) = timer_fd {
            poller.add(timer_fd, Event::none(crate::NOTIFY_KEY))?;
        }

        poller.add(
            event_fd,
            Event {
                key: crate::NOTIFY_KEY,
                readable: true,
                writable: false,
            },
        )?;

        Ok(poller)
    }

    pub fn add(&self, fd: RawFd, ev: Event) -> io::Result<()> {
        self.ctl(std::libc::EPOLL_CTL_ADD, fd, Some(ev))
    }

    pub fn modify(&self, fd: RawFd, ev: Event) -> io::Result<()> {
        self.ctl(std::libc::EPOLL_CTL_MOD, fd, Some(ev))
    }

    pub fn delete(&self, fd: RawFd) -> io::Result<()> {
        self.ctl(std::libc::EPOLL_CTL_DEL, fd, None)
    }

    pub fn wait(&self, events: &mut Events, timeout: Option<Duration>) -> io::Result<()> {
        if let Some(timer_fd) = self.timer_fd {
            let new_val = std::libc::itimerspec {
                it_interval: TS_ZERO,
                it_value: match timeout {
                    None => TS_ZERO,
                    Some(t) => std::libc::timespec {
                        tv_sec: t.as_secs() as libc::time_t,
                        tv_nsec: (t.subsec_nanos() as libc::c_long).into(),
                    },
                },
            };

            syscall!(syscall(
                libc::SYS_timerfd_settime,
                timer_fd as libc::c_int,
                0 as libc::c_int,
                &new_val as *const libc::itimerspec,
                ptr::null_mut() as *mut libc::itimerspec
            ))?;

            self.modify(
                timer_fd,
                Event {
                    key: crate::NOTIFY_KEY,
                    readable: true,
                    writable: false,
                },
            )?;
        }

        let timeout_ms = match (self.timer_fd, timeout) {
            (_, Some(t)) if t == Duration::from_secs(0) => 0,
            (None, Some(t)) => {
                let mut ms = t.as_millis().try_into().unwrap_or(std::i32::MAX);
                if Duration::from_millis(ms as u64) < t {
                    ms = ms.saturating_add(1);
                }
                ms
            }
            _ => -1,
        };

        let res = syscall!(epoll_wait(
            self.epoll_fd,
            events.list.as_mut_ptr() as *mut libc::epoll_event,
            events.list.len() as libc::c_int,
            timeout_ms as libc::c_int,
        ))?;
        events.len = res as usize;

        let mut buf = [0u8; 8];
        let _ = syscall!(read(
            self.event_fd,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len()
        ));
        self.modify(
            self.event_fd,
            Event {
                key: crate::NOTIFY_KEY,
                readable: true,
                writable: false,
            },
        )?;
        Ok(())
    }

    pub fn notify(&self) -> io::Result<()> {
        let buf: [u8; 8] = 1u64.to_ne_bytes();
        let _ = syscall!(write(
            self.event_fd,
            buf.as_ptr() as *const libc::c_void,
            buf.len()
        ));
        Ok(())
    }

    fn ctl(&self, op: libc::c_int, fd: RawFd, ev: Option<Event>) -> io::Result<()> {
        let mut ev = ev.map(|ev| {
            let mut flags = libc::EPOLLONESHOT;
            if ev.readable {
                flags |= read_flags();
            }
            if ev.writable {
                flags |= write_flags();
            }
            libc::epoll_event {
                events: flags as _,
                u64: ev.key as u64,
            }
        });
        syscall!(epoll_ctl(
            self.epoll_fd,
            op,
            fd,
            ev.as_mut()
                .map(|ev| ev as *mut libc::epoll_event)
                .unwrap_or(ptr::null_mut()),
        ))?;
        Ok(())
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        if let Some(timer_fd) = self.timer_fd {
            let _ = self.delete(timer_fd);
            let _ = syscall!(close(timer_fd));
        }
        let _ = self.delete(self.event_fd);
        let _ = syscall!(close(self.event_fd));
        let _ = syscall!(close(self.epoll_fd));
    }
}

const TS_ZERO: libc::timespec = libc::timespec {
    tv_sec: 0,
    tv_nsec: 0,
};

fn read_flags() -> libc::c_int {
    libc::EPOLLIN | libc::EPOLLRDHUP | libc::EPOLLHUP | libc::EPOLLERR | libc::EPOLLPRI
}

fn write_flags() -> libc::c_int {
    libc::EPOLLOUT | libc::EPOLLHUP | libc::EPOLLERR
}
