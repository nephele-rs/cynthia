#![forbid(unsafe_code)]

use core::fmt;
use core::ops::{Deref, DerefMut};

#[cfg_attr(any(target_arch = "x86_64", target_arch = "aarch64"), repr(align(128)))]
#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    repr(align(64))
)]
#[derive(Clone, Copy, Default, Hash, PartialEq, Eq)]
pub struct CacheAlignedPadding<T>(T);

impl<T> CacheAlignedPadding<T> {
    pub const fn new(t: T) -> CacheAlignedPadding<T> {
        CacheAlignedPadding(t)
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for CacheAlignedPadding<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for CacheAlignedPadding<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: fmt::Debug> fmt::Debug for CacheAlignedPadding<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CacheAlignedPadding").field(&self.0).finish()
    }
}

impl<T> From<T> for CacheAlignedPadding<T> {
    fn from(t: T) -> Self {
        CacheAlignedPadding::new(t)
    }
}
