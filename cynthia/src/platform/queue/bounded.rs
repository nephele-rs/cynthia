use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::platform::queue::{PopError, PushError};
use crate::utils::CacheAlignedPadding;

struct Slot<T> {
    stamp: AtomicUsize,
    value: UnsafeCell<MaybeUninit<T>>,
}

pub struct Bounded<T> {
    head: CacheAlignedPadding<AtomicUsize>,
    tail: CacheAlignedPadding<AtomicUsize>,
    buffer: Box<[Slot<T>]>,
    one_lap: usize,
    mark_bit: usize,
}

impl<T> Bounded<T> {
    pub fn new(cap: usize) -> Bounded<T> {
        assert!(cap > 0, "capacity must be positive");

        let head = 0;
        let tail = 0;

        let mut buffer = Vec::with_capacity(cap);
        for i in 0..cap {
            buffer.push(Slot {
                stamp: AtomicUsize::new(i),
                value: UnsafeCell::new(MaybeUninit::uninit()),
            });
        }

        let mark_bit = (cap + 1).next_power_of_two();
        let one_lap = mark_bit * 2;

        Bounded {
            buffer: buffer.into(),
            one_lap,
            mark_bit,
            head: CacheAlignedPadding::new(AtomicUsize::new(head)),
            tail: CacheAlignedPadding::new(AtomicUsize::new(tail)),
        }
    }

    pub fn push(&self, value: T) -> Result<(), PushError<T>> {
        let mut tail = self.tail.load(Ordering::Relaxed);

        loop {
            if tail & self.mark_bit != 0 {
                return Err(PushError::Closed(value));
            }

            let index = tail & (self.mark_bit - 1);
            let lap = tail & !(self.one_lap - 1);

            let slot = &self.buffer[index];
            let stamp = slot.stamp.load(Ordering::Acquire);

            if tail == stamp {
                let new_tail = if index + 1 < self.buffer.len() {
                    tail + 1
                } else {
                    lap.wrapping_add(self.one_lap)
                };

                match self.tail.compare_exchange_weak(
                    tail,
                    new_tail,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        unsafe {
                            slot.value.get().write(MaybeUninit::new(value));
                        }
                        slot.stamp.store(tail + 1, Ordering::Release);
                        return Ok(());
                    }
                    Err(t) => {
                        tail = t;
                    }
                }
            } else if stamp.wrapping_add(self.one_lap) == tail + 1 {
                crate::platform::queue::full_fence();
                let head = self.head.load(Ordering::Relaxed);

                if head.wrapping_add(self.one_lap) == tail {
                    return Err(PushError::Full(value));
                }

                tail = self.tail.load(Ordering::Relaxed);
            } else {
                thread::yield_now();
                tail = self.tail.load(Ordering::Relaxed);
            }
        }
    }

    pub fn pop(&self) -> Result<T, PopError> {
        let mut head = self.head.load(Ordering::Relaxed);

        loop {
            let index = head & (self.mark_bit - 1);
            let lap = head & !(self.one_lap - 1);

            let slot = &self.buffer[index];
            let stamp = slot.stamp.load(Ordering::Acquire);

            if head + 1 == stamp {
                let new = if index + 1 < self.buffer.len() {
                    head + 1
                } else {
                    lap.wrapping_add(self.one_lap)
                };

                match self.head.compare_exchange_weak(
                    head,
                    new,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        let value = unsafe { slot.value.get().read().assume_init() };
                        slot.stamp
                            .store(head.wrapping_add(self.one_lap), Ordering::Release);
                        return Ok(value);
                    }
                    Err(h) => {
                        head = h;
                    }
                }
            } else if stamp == head {
                crate::platform::queue::full_fence();
                let tail = self.tail.load(Ordering::Relaxed);

                if (tail & !self.mark_bit) == head {
                    if tail & self.mark_bit != 0 {
                        return Err(PopError::Closed);
                    } else {
                        return Err(PopError::Empty);
                    }
                }

                head = self.head.load(Ordering::Relaxed);
            } else {
                thread::yield_now();
                head = self.head.load(Ordering::Relaxed);
            }
        }
    }

    pub fn len(&self) -> usize {
        loop {
            let tail = self.tail.load(Ordering::SeqCst);
            let head = self.head.load(Ordering::SeqCst);

            if self.tail.load(Ordering::SeqCst) == tail {
                let hix = head & (self.mark_bit - 1);
                let tix = tail & (self.mark_bit - 1);

                return if hix < tix {
                    tix - hix
                } else if hix > tix {
                    self.buffer.len() - hix + tix
                } else if (tail & !self.mark_bit) == head {
                    0
                } else {
                    self.buffer.len()
                };
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::SeqCst);
        let tail = self.tail.load(Ordering::SeqCst);

        (tail & !self.mark_bit) == head
    }

    pub fn is_full(&self) -> bool {
        let tail = self.tail.load(Ordering::SeqCst);
        let head = self.head.load(Ordering::SeqCst);

        head.wrapping_add(self.one_lap) == tail & !self.mark_bit
    }

    pub fn capacity(&self) -> usize {
        self.buffer.len()
    }

    pub fn close(&self) -> bool {
        let tail = self.tail.fetch_or(self.mark_bit, Ordering::SeqCst);
        tail & self.mark_bit == 0
    }

    pub fn is_closed(&self) -> bool {
        self.tail.load(Ordering::SeqCst) & self.mark_bit != 0
    }
}

impl<T> Drop for Bounded<T> {
    fn drop(&mut self) {
        let hix = self.head.load(Ordering::Relaxed) & (self.mark_bit - 1);

        for i in 0..self.len() {
            let index = if hix + i < self.buffer.len() {
                hix + i
            } else {
                hix + i - self.buffer.len()
            };

            let slot = &self.buffer[index];
            unsafe {
                let value = slot.value.get().read().assume_init();
                drop(value);
            }
        }
    }
}
