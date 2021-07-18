#![allow(clippy::transmute_ptr_to_ptr)]

use std::fmt;
use std::mem::{self, MaybeUninit};

pub struct ReadBuf<'a> {
    buf: &'a mut [MaybeUninit<u8>],
    filled: usize,
    initialized: usize,
}

impl<'a> ReadBuf<'a> {
    #[inline]
    pub fn new(buf: &'a mut [u8]) -> ReadBuf<'a> {
        let initialized = buf.len();
        let buf = unsafe { mem::transmute::<&mut [u8], &mut [MaybeUninit<u8>]>(buf) };
        ReadBuf {
            buf,
            filled: 0,
            initialized,
        }
    }

    #[inline]
    pub fn uninit(buf: &'a mut [MaybeUninit<u8>]) -> ReadBuf<'a> {
        ReadBuf {
            buf,
            filled: 0,
            initialized: 0,
        }
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    #[inline]
    pub fn filled(&self) -> &[u8] {
        let slice = &self.buf[..self.filled];
        unsafe { mem::transmute::<&[MaybeUninit<u8>], &[u8]>(slice) }
    }

    #[inline]
    pub fn filled_mut(&mut self) -> &mut [u8] {
        let slice = &mut self.buf[..self.filled];
        unsafe { mem::transmute::<&mut [MaybeUninit<u8>], &mut [u8]>(slice) }
    }

    #[inline]
    pub fn take(&mut self, n: usize) -> ReadBuf<'_> {
        let max = std::cmp::min(self.remaining(), n);
        unsafe { ReadBuf::uninit(&mut self.unfilled_mut()[..max]) }
    }

    #[inline]
    pub fn initialized(&self) -> &[u8] {
        let slice = &self.buf[..self.initialized];
        unsafe { mem::transmute::<&[MaybeUninit<u8>], &[u8]>(slice) }
    }

    #[inline]
    pub fn initialized_mut(&mut self) -> &mut [u8] {
        let slice = &mut self.buf[..self.initialized];
        unsafe { mem::transmute::<&mut [MaybeUninit<u8>], &mut [u8]>(slice) }
    }

    #[inline]
    pub unsafe fn unfilled_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        &mut self.buf[self.filled..]
    }

    #[inline]
    pub fn initialize_unfilled(&mut self) -> &mut [u8] {
        self.initialize_unfilled_to(self.remaining())
    }

    #[inline]
    pub fn initialize_unfilled_to(&mut self, n: usize) -> &mut [u8] {
        assert!(self.remaining() >= n, "n overflows remaining");

        let end = self.filled + n;

        if self.initialized < end {
            unsafe {
                self.buf[self.initialized..end]
                    .as_mut_ptr()
                    .write_bytes(0, end - self.initialized);
            }
            self.initialized = end;
        }

        let slice = &mut self.buf[self.filled..end];
        unsafe { mem::transmute::<&mut [MaybeUninit<u8>], &mut [u8]>(slice) }
    }

    #[inline]
    pub fn remaining(&self) -> usize {
        self.capacity() - self.filled
    }

    #[inline]
    pub fn clear(&mut self) {
        self.filled = 0;
    }

    #[inline]
    pub fn advance(&mut self, n: usize) {
        let new = self.filled.checked_add(n).expect("filled overflow");
        self.set_filled(new);
    }

    #[inline]
    pub fn set_filled(&mut self, n: usize) {
        assert!(
            n <= self.initialized,
            "filled must not become larger than initialized"
        );
        self.filled = n;
    }

    #[inline]
    pub unsafe fn assume_init(&mut self, n: usize) {
        let new = self.filled + n;
        if new > self.initialized {
            self.initialized = new;
        }
    }

    #[inline]
    pub fn put_slice(&mut self, buf: &[u8]) {
        assert!(
            self.remaining() >= buf.len(),
            "buf.len() must fit in remaining()"
        );

        let amt = buf.len();
        let end = self.filled + amt;

        unsafe {
            self.buf[self.filled..end]
                .as_mut_ptr()
                .cast::<u8>()
                .copy_from_nonoverlapping(buf.as_ptr(), amt);
        }

        if self.initialized < end {
            self.initialized = end;
        }
        self.filled = end;
    }
}

impl fmt::Debug for ReadBuf<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadBuf")
            .field("filled", &self.filled)
            .field("initialized", &self.initialized)
            .field("capacity", &self.capacity())
            .finish()
    }
}
