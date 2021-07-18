use std::ffi::OsStr;
use std::io;
use std::os::unix::process::CommandExt as _;

use crate::process::Command;

pub trait CommandExt {
    fn uid(&mut self, id: u32) -> &mut Command;

    fn gid(&mut self, id: u32) -> &mut Command;

    unsafe fn pre_exec<F>(&mut self, f: F) -> &mut Command
    where
        F: FnMut() -> io::Result<()> + Send + Sync + 'static;

    fn exec(&mut self) -> io::Error;

    fn arg0<S>(&mut self, arg: S) -> &mut Command
    where
        S: AsRef<OsStr>;
}

impl CommandExt for Command {
    fn uid(&mut self, id: u32) -> &mut Command {
        self.inner.uid(id);
        self
    }

    fn gid(&mut self, id: u32) -> &mut Command {
        self.inner.gid(id);
        self
    }

    unsafe fn pre_exec<F>(&mut self, f: F) -> &mut Command
    where
        F: FnMut() -> io::Result<()> + Send + Sync + 'static,
    {
        self.inner.pre_exec(f);
        self
    }

    fn exec(&mut self) -> io::Error {
        self.inner.exec()
    }

    fn arg0<S>(&mut self, _arg: S) -> &mut Command
    where
        S: AsRef<OsStr>,
    {
        self
    }
}
