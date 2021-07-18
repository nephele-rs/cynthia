use std::os::windows::process::CommandExt as _;

use crate::Command;

pub trait CommandExt {
    fn creation_flags(&mut self, flags: u32) -> &mut Command;
}

impl CommandExt for Command {
    fn creation_flags(&mut self, flags: u32) -> &mut Command {
        self.inner.creation_flags(flags);
        self
    }
}
