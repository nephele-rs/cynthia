use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::{ptr, thread};

use crate::platform::queue::{PopError, PushError};
use crate::utils::CacheAlignedPadding;

const WRITE: usize = 1;
const READ: usize = 2;
const DESTROY: usize = 4;

const LAP: usize = 32;
const BLOCK_CAP: usize = LAP - 1;
const SHIFT: usize = 1;
const MARK_BIT: usize = 1;

struct Slot<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    state: AtomicUsize,
}

impl<T> Slot<T> {
    const UNINIT: Slot<T> = Slot {
        value: UnsafeCell::new(MaybeUninit::uninit()),
        state: AtomicUsize::new(0),
    };

    fn wait_write(&self) {
        while self.state.load(Ordering::Acquire) & WRITE == 0 {
            thread::yield_now();
        }
    }
}

struct Block<T> {
    next: AtomicPtr<Block<T>>,
    slots: [Slot<T>; BLOCK_CAP],
}

impl<T> Block<T> {
    fn new() -> Block<T> {
        Block {
            next: AtomicPtr::new(ptr::null_mut()),
            slots: [Slot::UNINIT; BLOCK_CAP],
        }
    }

    fn wait_next(&self) -> *mut Block<T> {
        loop {
            let next = self.next.load(Ordering::Acquire);
            if !next.is_null() {
                return next;
            }
            thread::yield_now();
        }
    }

    unsafe fn destroy(this: *mut Block<T>, start: usize) {
        for i in start..BLOCK_CAP - 1 {
            let slot = (*this).slots.get_unchecked(i);

            if slot.state.load(Ordering::Acquire) & READ == 0
                && slot.state.fetch_or(DESTROY, Ordering::AcqRel) & READ == 0
            {
                return;
            }
        }

        drop(Box::from_raw(this));
    }
}

struct Position<T> {
    index: AtomicUsize,
    block: AtomicPtr<Block<T>>,
}

pub struct Unbounded<T> {
    head: CacheAlignedPadding<Position<T>>,
    tail: CacheAlignedPadding<Position<T>>,
}

impl<T> Unbounded<T> {
    pub fn new() -> Unbounded<T> {
        Unbounded {
            head: CacheAlignedPadding::new(Position {
                block: AtomicPtr::new(ptr::null_mut()),
                index: AtomicUsize::new(0),
            }),
            tail: CacheAlignedPadding::new(Position {
                block: AtomicPtr::new(ptr::null_mut()),
                index: AtomicUsize::new(0),
            }),
        }
    }

    pub fn push(&self, value: T) -> Result<(), PushError<T>> {
        let mut tail = self.tail.index.load(Ordering::Acquire);
        let mut block = self.tail.block.load(Ordering::Acquire);
        let mut next_block = None;

        loop {
            if tail & MARK_BIT != 0 {
                return Err(PushError::Closed(value));
            }

            let offset = (tail >> SHIFT) % LAP;

            if offset == BLOCK_CAP {
                thread::yield_now();
                tail = self.tail.index.load(Ordering::Acquire);
                block = self.tail.block.load(Ordering::Acquire);
                continue;
            }

            if offset + 1 == BLOCK_CAP && next_block.is_none() {
                next_block = Some(Box::new(Block::<T>::new()));
            }

            if block.is_null() {
                let new = Box::into_raw(Box::new(Block::<T>::new()));

                if self
                    .tail
                    .block
                    .compare_exchange(block, new, Ordering::Release, Ordering::Relaxed)
                    .is_ok()
                {
                    self.head.block.store(new, Ordering::Release);
                    block = new;
                } else {
                    next_block = unsafe { Some(Box::from_raw(new)) };
                    tail = self.tail.index.load(Ordering::Acquire);
                    block = self.tail.block.load(Ordering::Acquire);
                    continue;
                }
            }

            let new_tail = tail + (1 << SHIFT);

            match self.tail.index.compare_exchange_weak(
                tail,
                new_tail,
                Ordering::SeqCst,
                Ordering::Acquire,
            ) {
                Ok(_) => unsafe {
                    if offset + 1 == BLOCK_CAP {
                        let next_block = Box::into_raw(next_block.unwrap());
                        self.tail.block.store(next_block, Ordering::Release);
                        self.tail.index.fetch_add(1 << SHIFT, Ordering::Release);
                        (*block).next.store(next_block, Ordering::Release);
                    }

                    let slot = (*block).slots.get_unchecked(offset);
                    slot.value.get().write(MaybeUninit::new(value));
                    slot.state.fetch_or(WRITE, Ordering::Release);
                    return Ok(());
                },
                Err(t) => {
                    tail = t;
                    block = self.tail.block.load(Ordering::Acquire);
                }
            }
        }
    }

    pub fn pop(&self) -> Result<T, PopError> {
        let mut head = self.head.index.load(Ordering::Acquire);
        let mut block = self.head.block.load(Ordering::Acquire);

        loop {
            let offset = (head >> SHIFT) % LAP;

            if offset == BLOCK_CAP {
                thread::yield_now();
                head = self.head.index.load(Ordering::Acquire);
                block = self.head.block.load(Ordering::Acquire);
                continue;
            }

            let mut new_head = head + (1 << SHIFT);

            if new_head & MARK_BIT == 0 {
                crate::platform::queue::full_fence();
                let tail = self.tail.index.load(Ordering::Relaxed);

                if head >> SHIFT == tail >> SHIFT {
                    if tail & MARK_BIT != 0 {
                        return Err(PopError::Closed);
                    } else {
                        return Err(PopError::Empty);
                    }
                }

                if (head >> SHIFT) / LAP != (tail >> SHIFT) / LAP {
                    new_head |= MARK_BIT;
                }
            }

            if block.is_null() {
                thread::yield_now();
                head = self.head.index.load(Ordering::Acquire);
                block = self.head.block.load(Ordering::Acquire);
                continue;
            }

            match self.head.index.compare_exchange_weak(
                head,
                new_head,
                Ordering::SeqCst,
                Ordering::Acquire,
            ) {
                Ok(_) => unsafe {
                    if offset + 1 == BLOCK_CAP {
                        let next = (*block).wait_next();
                        let mut next_index = (new_head & !MARK_BIT).wrapping_add(1 << SHIFT);
                        if !(*next).next.load(Ordering::Relaxed).is_null() {
                            next_index |= MARK_BIT;
                        }

                        self.head.block.store(next, Ordering::Release);
                        self.head.index.store(next_index, Ordering::Release);
                    }

                    let slot = (*block).slots.get_unchecked(offset);
                    slot.wait_write();
                    let value = slot.value.get().read().assume_init();

                    if offset + 1 == BLOCK_CAP {
                        Block::destroy(block, 0);
                    } else if slot.state.fetch_or(READ, Ordering::AcqRel) & DESTROY != 0 {
                        Block::destroy(block, offset + 1);
                    }

                    return Ok(value);
                },
                Err(h) => {
                    head = h;
                    block = self.head.block.load(Ordering::Acquire);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        loop {
            let mut tail = self.tail.index.load(Ordering::SeqCst);
            let mut head = self.head.index.load(Ordering::SeqCst);

            if self.tail.index.load(Ordering::SeqCst) == tail {
                tail &= !((1 << SHIFT) - 1);
                head &= !((1 << SHIFT) - 1);

                if (tail >> SHIFT) & (LAP - 1) == LAP - 1 {
                    tail = tail.wrapping_add(1 << SHIFT);
                }
                if (head >> SHIFT) & (LAP - 1) == LAP - 1 {
                    head = head.wrapping_add(1 << SHIFT);
                }

                let lap = (head >> SHIFT) / LAP;
                tail = tail.wrapping_sub((lap * LAP) << SHIFT);
                head = head.wrapping_sub((lap * LAP) << SHIFT);

                tail >>= SHIFT;
                head >>= SHIFT;

                return tail - head - tail / LAP;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        let head = self.head.index.load(Ordering::SeqCst);
        let tail = self.tail.index.load(Ordering::SeqCst);
        head >> SHIFT == tail >> SHIFT
    }

    pub fn is_full(&self) -> bool {
        false
    }

    pub fn close(&self) -> bool {
        let tail = self.tail.index.fetch_or(MARK_BIT, Ordering::SeqCst);
        tail & MARK_BIT == 0
    }

    pub fn is_closed(&self) -> bool {
        self.tail.index.load(Ordering::SeqCst) & MARK_BIT != 0
    }
}

impl<T> Drop for Unbounded<T> {
    fn drop(&mut self) {
        let mut head = self.head.index.load(Ordering::Relaxed);
        let mut tail = self.tail.index.load(Ordering::Relaxed);
        let mut block = self.head.block.load(Ordering::Relaxed);

        head &= !((1 << SHIFT) - 1);
        tail &= !((1 << SHIFT) - 1);

        unsafe {
            while head != tail {
                let offset = (head >> SHIFT) % LAP;

                if offset < BLOCK_CAP {
                    let slot = (*block).slots.get_unchecked(offset);
                    let value = slot.value.get().read().assume_init();
                    drop(value);
                } else {
                    let next = (*block).next.load(Ordering::Relaxed);
                    drop(Box::from_raw(block));
                    block = next;
                }

                head = head.wrapping_add(1 << SHIFT);
            }

            if !block.is_null() {
                drop(Box::from_raw(block));
            }
        }
    }
}
