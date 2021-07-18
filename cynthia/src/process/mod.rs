use std::ffi::OsStr;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::{fmt, thread};
pub use std::process::{ExitStatus, Output, Stdio};

#[cfg(unix)]
use crate::io::Async;

#[cfg(windows)]
use crate::blocking::Unblock;

use crate::future::{future, prelude::*, swap};
use crate::platform::event::Event;
use once_cell::sync::Lazy;

#[cfg(unix)]
pub mod unix;

#[cfg(windows)]
pub mod windows;

static SIGCHLD: Event = Event::new();

#[derive(Debug)]
pub struct ChildStdin(
    #[cfg(windows)] Unblock<std::process::ChildStdin>,
    #[cfg(unix)] Async<std::process::ChildStdin>,
);

impl swap::AsyncWrite for ChildStdin {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<swap::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<swap::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<swap::Result<()>> {
        Pin::new(&mut self.0).poll_close(cx)
    }
}

#[derive(Debug)]
pub struct ChildStdout(
    #[cfg(windows)] Unblock<std::process::ChildStdout>,
    #[cfg(unix)] Async<std::process::ChildStdout>,
);

impl swap::AsyncRead for ChildStdout {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<swap::Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

#[derive(Debug)]
pub struct ChildStderr(
    #[cfg(windows)] Unblock<std::process::ChildStderr>,
    #[cfg(unix)] Async<std::process::ChildStderr>,
);

impl swap::AsyncRead for ChildStderr {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<swap::Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

#[derive(Debug)]
pub struct Command {
    inner: std::process::Command,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
    reap_on_drop: bool,
    kill_on_drop: bool,
}

impl Command {
    pub fn new<S: AsRef<OsStr>>(program: S) -> Command {
        Command {
            inner: std::process::Command::new(program),
            stdin: None,
            stdout: None,
            stderr: None,
            reap_on_drop: true,
            kill_on_drop: false,
        }
    }

    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Command {
        self.inner.arg(arg);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.inner.args(args);
        self
    }

    pub fn env<K, V>(&mut self, key: K, val: V) -> &mut Command
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.env(key, val);
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Command
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.envs(vars);
        self
    }

    pub fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Command {
        self.inner.env_remove(key);
        self
    }

    pub fn env_clear(&mut self) -> &mut Command {
        self.inner.env_clear();
        self
    }

    pub fn current_dir<P: AsRef<Path>>(&mut self, dir: P) -> &mut Command {
        self.inner.current_dir(dir);
        self
    }

    pub fn stdin<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Command {
        self.stdin = Some(cfg.into());
        self
    }

    pub fn stdout<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Command {
        self.stdout = Some(cfg.into());
        self
    }

    pub fn stderr<T: Into<Stdio>>(&mut self, cfg: T) -> &mut Command {
        self.stderr = Some(cfg.into());
        self
    }

    pub fn reap_on_drop(&mut self, reap_on_drop: bool) -> &mut Command {
        self.reap_on_drop = reap_on_drop;
        self
    }

    pub fn kill_on_drop(&mut self, kill_on_drop: bool) -> &mut Command {
        self.kill_on_drop = kill_on_drop;
        self
    }

    pub fn spawn(&mut self) -> swap::Result<Child> {
        let (stdin, stdout, stderr) = (self.stdin.take(), self.stdout.take(), self.stderr.take());
        self.inner.stdin(stdin.unwrap_or(Stdio::inherit()));
        self.inner.stdout(stdout.unwrap_or(Stdio::inherit()));
        self.inner.stderr(stderr.unwrap_or(Stdio::inherit()));

        Child::new(self)
    }

    pub fn status(&mut self) -> impl Future<Output = swap::Result<ExitStatus>> {
        let child = self.spawn();
        async { child?.status().await }
    }

    pub fn output(&mut self) -> impl Future<Output = swap::Result<Output>> {
        let (stdin, stdout, stderr) = (self.stdin.take(), self.stdout.take(), self.stderr.take());
        self.inner.stdin(stdin.unwrap_or(Stdio::null()));
        self.inner.stdout(stdout.unwrap_or(Stdio::piped()));
        self.inner.stderr(stderr.unwrap_or(Stdio::piped()));

        let child = Child::new(self);
        async { child?.output().await }
    }
}

struct ChildGuard {
    inner: Option<std::process::Child>,
    reap_on_drop: bool,
}

impl ChildGuard {
    fn get_mut(&mut self) -> &mut std::process::Child {
        self.inner.as_mut().unwrap()
    }
}

pub struct Child {
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    child: Arc<Mutex<ChildGuard>>,
    kill_on_drop: bool,
}

impl Child {
    fn new(cmd: &mut Command) -> swap::Result<Child> {
        let mut child = cmd.inner.spawn()?;

        let stdin = child.stdin.take().map(wrap).transpose()?.map(ChildStdin);
        let stdout = child.stdout.take().map(wrap).transpose()?.map(ChildStdout);
        let stderr = child.stderr.take().map(wrap).transpose()?.map(ChildStderr);

        cfg_if::cfg_if! {
            if #[cfg(windows)] {
                use std::os::windows::io::AsRawHandle;
                use std::sync::mpsc;

                use winapi::um::{
                    winbase::{RegisterWaitForSingleObject, INFINITE},
                    winnt::{BOOLEAN, HANDLE, PVOID, WT_EXECUTEINWAITTHREAD, WT_EXECUTEONLYONCE},
                };

                static CALLBACK: Lazy<(mpsc::SyncSender<()>, Mutex<mpsc::Receiver<()>>)> =
                    Lazy::new(|| {
                        let (s, r) = mpsc::sync_channel(1);
                        (s, Mutex::new(r))
                    });

                unsafe extern "system" fn callback(_: PVOID, _: BOOLEAN) {
                    CALLBACK.0.try_send(()).ok();
                }

                let mut wait_object = std::ptr::null_mut();
                let ret = unsafe {
                    RegisterWaitForSingleObject(
                        &mut wait_object,
                        child.as_raw_handle() as HANDLE,
                        Some(callback),
                        std::ptr::null_mut(),
                        INFINITE,
                        WT_EXECUTEINWAITTHREAD | WT_EXECUTEONLYONCE,
                    )
                };
                if ret == 0 {
                    return Err(io::Error::last_os_error());
                }

                fn wait_sigchld() {
                    CALLBACK.1.lock().unwrap().recv().ok();
                }

                fn wrap<T>(io: T) -> io::Result<Unblock<T>> {
                    Ok(Unblock::new(io))
                }

            } else if #[cfg(unix)] {
                static SIGNALS: Lazy<signal_hook::iterator::Signals> = Lazy::new(|| {
                    signal_hook::iterator::Signals::new(&[signal_hook::SIGCHLD])
                        .expect("cannot set signal handler for SIGCHLD")
                });

                Lazy::force(&SIGNALS);

                fn wait_sigchld() {
                    SIGNALS.forever().next();
                }

                fn wrap<T: std::os::unix::io::AsRawFd>(io: T) -> swap::Result<Async<T>> {
                    Async::new(io)
                }
            }
        }

        static ZOMBIES: Lazy<Mutex<Vec<std::process::Child>>> = Lazy::new(|| {
            thread::Builder::new()
                .name("cynthia-process".to_string())
                .spawn(move || loop {
                    wait_sigchld();

                    SIGCHLD.notify(std::usize::MAX);

                    let mut zombies = ZOMBIES.lock().unwrap();
                    let mut i = 0;
                    while i < zombies.len() {
                        if let Ok(None) = zombies[i].try_wait() {
                            i += 1;
                        } else {
                            zombies.swap_remove(i);
                        }
                    }
                })
                .expect("cannot spawn cynthia-process thread");

            Mutex::new(Vec::new())
        });

        Lazy::force(&ZOMBIES);

        impl Drop for ChildGuard {
            fn drop(&mut self) {
                if self.reap_on_drop {
                    let mut zombies = ZOMBIES.lock().unwrap();
                    if let Ok(None) = self.get_mut().try_wait() {
                        zombies.push(self.inner.take().unwrap());
                    }
                }
            }
        }

        Ok(Child {
            stdin,
            stdout,
            stderr,
            child: Arc::new(Mutex::new(ChildGuard {
                inner: Some(child),
                reap_on_drop: cmd.reap_on_drop,
            })),
            kill_on_drop: cmd.kill_on_drop,
        })
    }

    pub fn id(&self) -> u32 {
        self.child.lock().unwrap().get_mut().id()
    }

    pub fn kill(&mut self) -> swap::Result<()> {
        self.child.lock().unwrap().get_mut().kill()
    }

    pub fn try_status(&mut self) -> swap::Result<Option<ExitStatus>> {
        self.child.lock().unwrap().get_mut().try_wait()
    }

    pub fn status(&mut self) -> impl Future<Output = swap::Result<ExitStatus>> {
        self.stdin.take();
        let child = self.child.clone();

        async move {
            let mut listener = None;
            loop {
                if let Some(status) = child.lock().unwrap().get_mut().try_wait()? {
                    return Ok(status);
                }
                match listener.take() {
                    None => listener = Some(SIGCHLD.listen()),
                    Some(listener) => listener.await,
                }
            }
        }
    }

    pub fn output(mut self) -> impl Future<Output = swap::Result<Output>> {
        let status = self.status();

        let stdout = self.stdout.take();
        let stdout = async move {
            let mut v = Vec::new();
            if let Some(mut s) = stdout {
                s.read_to_end(&mut v).await?;
            }
            swap::Result::Ok(v)
        };

        let stderr = self.stderr.take();
        let stderr = async move {
            let mut v = Vec::new();
            if let Some(mut s) = stderr {
                s.read_to_end(&mut v).await?;
            }
            swap::Result::Ok(v)
        };

        async move {
            let (stdout, stderr) = future::try_zip(stdout, stderr).await?;
            let status = status.await?;
            Ok(Output {
                status,
                stdout,
                stderr,
            })
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if self.kill_on_drop {
            self.kill().ok();
        }
    }
}

impl fmt::Debug for Child {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Child")
            .field("stdin", &self.stdin)
            .field("stdout", &self.stdout)
            .field("stderr", &self.stderr)
            .finish()
    }
}
