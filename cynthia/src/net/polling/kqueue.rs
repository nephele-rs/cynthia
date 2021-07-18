use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::ptr;
use std::time::Duration;

use crate::net::polling::Event;

pub struct Events {
    list: Box<[libc::kevent]>,
    len: usize,
}

unsafe impl Send for Events {}

impl Events {
    pub fn new() -> Events {
        let ev = libc::kevent {
            ident: 0 as _,
            filter: 0,
            flags: 0,
            fflags: 0,
            data: 0,
            udata: 0 as _,
        };
        let list = vec![ev; 1000].into_boxed_slice();
        let len = 0;
        Events { list, len }
    }

    pub fn iter(&self) -> impl Iterator<Item = Event> + '_ {
        self.list[..self.len].iter().map(|ev| Event {
            key: ev.udata as usize,
            readable: ev.filter == libc::EVFILT_READ,
            writable: ev.filter == libc::EVFILT_WRITE
                || (ev.filter == libc::EVFILT_READ && (ev.flags & libc::EV_EOF) != 0),
        })
    }
}

#[derive(Debug)]
pub struct Poller {
    kqueue_fd: RawFd,
    read_stream: UnixStream,
    write_stream: UnixStream,
}

impl Poller {
    pub fn new() -> io::Result<Poller> {
        let kqueue_fd = syscall!(kqueue())?;
        syscall!(fcntl(kqueue_fd, libc::F_SETFD, libc::FD_CLOEXEC))?;

        let (read_stream, write_stream) = UnixStream::pair()?;
        read_stream.set_nonblocking(true)?;
        write_stream.set_nonblocking(true)?;

        let poller = Poller {
            kqueue_fd,
            read_stream,
            write_stream,
        };
        poller.add(
            poller.read_stream.as_raw_fd(),
            Event {
                key: crate::net::polling::NOTIFY_KEY,
                readable: true,
                writable: false,
            },
        )?;

        Ok(poller)
    }

    pub fn add(&self, fd: RawFd, ev: Event) -> io::Result<()> {
        self.modify(fd, ev)
    }

    pub fn modify(&self, fd: RawFd, ev: Event) -> io::Result<()> {
        let read_flags = if ev.readable {
            libc::EV_ADD | libc::EV_ONESHOT
        } else {
            libc::EV_DELETE
        };
        let write_flags = if ev.writable {
            libc::EV_ADD | libc::EV_ONESHOT
        } else {
            libc::EV_DELETE
        };

        let changelist = [
            libc::kevent {
                ident: fd as _,
                filter: libc::EVFILT_READ,
                flags: read_flags | libc::EV_RECEIPT,
                fflags: 0,
                data: 0,
                udata: ev.key as _,
            },
            libc::kevent {
                ident: fd as _,
                filter: libc::EVFILT_WRITE,
                flags: write_flags | libc::EV_RECEIPT,
                fflags: 0,
                data: 0,
                udata: ev.key as _,
            },
        ];

        let mut eventlist = changelist;
        syscall!(kevent(
            self.kqueue_fd,
            changelist.as_ptr() as *const libc::kevent,
            changelist.len() as _,
            eventlist.as_mut_ptr() as *mut libc::kevent,
            eventlist.len() as _,
            ptr::null(),
        ))?;

        for ev in &eventlist {
            if (ev.flags & libc::EV_ERROR) != 0
                && ev.data != 0
                && ev.data != libc::ENOENT as isize
                && ev.data != libc::EPIPE as isize
            {
                return Err(io::Error::from_raw_os_error(ev.data as _));
            }
        }

        Ok(())
    }

    pub fn delete(&self, fd: RawFd) -> io::Result<()> {
        self.modify(fd, Event::none(0))
    }

    pub fn wait(&self, events: &mut Events, timeout: Option<Duration>) -> io::Result<()> {
        let timeout = timeout.map(|t| libc::timespec {
            tv_sec: t.as_secs() as libc::time_t,
            tv_nsec: t.subsec_nanos() as libc::c_long,
        });

        let changelist = [];
        let eventlist = &mut events.list;
        let res = syscall!(kevent(
            self.kqueue_fd,
            changelist.as_ptr() as *const libc::kevent,
            changelist.len() as _,
            eventlist.as_mut_ptr() as *mut libc::kevent,
            eventlist.len() as _,
            match &timeout {
                None => ptr::null(),
                Some(t) => t,
            }
        ))?;
        events.len = res as usize;

        while (&self.read_stream).read(&mut [0; 64]).is_ok() {}
        self.modify(
            self.read_stream.as_raw_fd(),
            Event {
                key: crate::net::polling::NOTIFY_KEY,
                readable: true,
                writable: false,
            },
        )?;

        Ok(())
    }

    pub fn notify(&self) -> io::Result<()> {
        let _ = (&self.write_stream).write(&[1]);
        Ok(())
    }
}

impl Drop for Poller {
    fn drop(&mut self) {
        let _ = self.delete(self.read_stream.as_raw_fd());
        let _ = syscall!(close(self.kqueue_fd));
    }
}
