use std::fmt;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr;

use crossbeam_queue::SegQueue;
use stable_deref_trait::StableDeref;

use super::alloc::{Allocate, Realloc};

pub struct BytePool<T = Vec<u8>>
where
    T: Allocate,
{
    small_bask: SegQueue<T>,
    medium_bask: SegQueue<T>,
    large_bask: SegQueue<T>,
}

pub struct ByteBuffer<'a, T: Allocate = Vec<u8>> {
    data: mem::ManuallyDrop<T>,
    pool: &'a BytePool<T>,
}

const LOW_LINE: usize = 2 * 1024;
const HIGH_LINE: usize = 64 * 1024;

impl<T: Allocate + fmt::Debug> fmt::Debug for ByteBuffer<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ByteBuffer")
            .field("data", &self.data)
            .finish()
    }
}

impl<T: Allocate> Default for BytePool<T> {
    fn default() -> Self {
        BytePool::<T> {
            small_bask: SegQueue::new(),
            medium_bask: SegQueue::new(),
            large_bask: SegQueue::new(),
        }
    }
}

impl<T: Allocate> BytePool<T> {
    pub fn new() -> Self {
        BytePool::default()
    }

    pub fn alloc(&self, size: usize) -> ByteBuffer<'_, T> {
        let list = if size <= LOW_LINE {
            &self.small_bask
        } else if size <= HIGH_LINE {
            &self.medium_bask
        } else {
            &self.large_bask
        };
        if let Some(el) = list.pop() {
            if el.capacity() == size {
                return ByteBuffer::new(el, self);
            } else {
                list.push(el);
            }
        }

        let data = T::alloc(size);
        ByteBuffer::new(data, self)
    }

    fn push_raw_block(&self, buffer: T) {
        let size = buffer.capacity();
        if size <= LOW_LINE {
            self.small_bask.push(buffer);
        } else if size <= HIGH_LINE {
            self.medium_bask.push(buffer);
        } else {
            self.large_bask.push(buffer);
        }
    }
}

impl<'a, T: Allocate> Drop for ByteBuffer<'a, T> {
    fn drop(&mut self) {
        let data = mem::ManuallyDrop::into_inner(unsafe { ptr::read(&self.data) });
        self.pool.push_raw_block(data);
    }
}

impl<'a, T: Allocate> ByteBuffer<'a, T> {
    fn new(data: T, pool: &'a BytePool<T>) -> Self {
        ByteBuffer {
            data: mem::ManuallyDrop::new(data),
            pool,
        }
    }

    pub fn size(&self) -> usize {
        self.data.capacity()
    }
}

impl<'a, T: Allocate + Realloc> ByteBuffer<'a, T> {
    pub fn realloc(&mut self, new_size: usize) {
        self.data.realloc(new_size);
    }
}

impl<'a, T: Allocate> Deref for ByteBuffer<'a, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.data.deref()
    }
}

impl<'a, T: Allocate> DerefMut for ByteBuffer<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data.deref_mut()
    }
}

unsafe impl<'a, T: StableDeref + Allocate> StableDeref for ByteBuffer<'a, T> {}
