#[cfg(unix)]
use std::os::unix::io::RawFd;

#[cfg(windows)]
use std::os::windows::io::RawSocket;

cfg_if::cfg_if! {
    if #[cfg(windows)] {
        pub type TransportFd = RawSocket;
    } else if #[cfg(unix)] {
        pub type TransportFd = RawFd;
    }
}
