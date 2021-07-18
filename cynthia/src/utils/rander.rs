#![forbid(unsafe_code)]

use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::{Bound, RangeBounds};
use std::thread;

#[cfg(target_arch = "wasm32")]
use instant::Instant;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

#[derive(Debug)]
pub struct Rander(Cell<u64>);

impl Default for Rander {
    #[inline]
    fn default() -> Rander {
        Rander::new()
    }
}

impl Clone for Rander {
    fn clone(&self) -> Rander {
        Rander::with_seed(self.gen_u64())
    }
}

impl Rander {
    #[inline]
    fn gen_u32(&self) -> u32 {
        let s = self.0.get();
        self.0.set(
            s.wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407),
        );
        (((s ^ (s >> 18)) >> 27) as u32).rotate_right((s >> 59) as u32)
    }

    #[inline]
    fn gen_u64(&self) -> u64 {
        ((self.gen_u32() as u64) << 32) | (self.gen_u32() as u64)
    }

    #[inline]
    fn gen_u128(&self) -> u128 {
        ((self.gen_u64() as u128) << 64) | (self.gen_u64() as u128)
    }

    #[inline]
    fn gen_mod_u32(&self, n: u32) -> u32 {
        let mut r = self.gen_u32();
        let mut hi = mul_high_u32(r, n);
        let mut lo = r.wrapping_mul(n);
        if lo < n {
            let t = n.wrapping_neg() % n;
            while lo < t {
                r = self.gen_u32();
                hi = mul_high_u32(r, n);
                lo = r.wrapping_mul(n);
            }
        }
        hi
    }

    #[inline]
    fn gen_mod_u64(&self, n: u64) -> u64 {
        let mut r = self.gen_u64();
        let mut hi = mul_high_u64(r, n);
        let mut lo = r.wrapping_mul(n);
        if lo < n {
            let t = n.wrapping_neg() % n;
            while lo < t {
                r = self.gen_u64();
                hi = mul_high_u64(r, n);
                lo = r.wrapping_mul(n);
            }
        }
        hi
    }

    #[inline]
    fn gen_mod_u128(&self, n: u128) -> u128 {
        let mut r = self.gen_u128();
        let mut hi = mul_high_u128(r, n);
        let mut lo = r.wrapping_mul(n);
        if lo < n {
            let t = n.wrapping_neg() % n;
            while lo < t {
                r = self.gen_u128();
                hi = mul_high_u128(r, n);
                lo = r.wrapping_mul(n);
            }
        }
        hi
    }
}

thread_local! {
    static RH: Rander = Rander(Cell::new({
        let mut hasher = DefaultHasher::new();
        Instant::now().hash(&mut hasher);
        thread::current().id().hash(&mut hasher);
        let hash = hasher.finish();
        (hash << 1) | 1
    }));
}

#[inline]
fn mul_high_u32(a: u32, b: u32) -> u32 {
    (((a as u64) * (b as u64)) >> 32) as u32
}

#[inline]
fn mul_high_u64(a: u64, b: u64) -> u64 {
    (((a as u128) * (b as u128)) >> 64) as u64
}

#[inline]
fn mul_high_u128(a: u128, b: u128) -> u128 {
    let a_lo = a as u64 as u128;
    let a_hi = (a >> 64) as u64 as u128;
    let b_lo = b as u64 as u128;
    let b_hi = (b >> 64) as u64 as u128;
    let carry = (a_lo * b_lo) >> 64;
    let carry = ((a_hi * b_lo) as u64 as u128 + (a_lo * b_hi) as u64 as u128 + carry) >> 64;
    a_hi * b_hi + ((a_hi * b_lo) >> 64) + ((a_lo * b_hi) >> 64) + carry
}

macro_rules! rand_integer {
    ($t:tt, $gen:tt, $mod:tt, $doc:tt) => {
        #[doc = $doc]
        #[inline]
        pub fn $t(&self, range: impl RangeBounds<$t>) -> $t {
            let panic_empty_range = || {
                panic!(
                    "empty range: {:?}..{:?}",
                    range.start_bound(),
                    range.end_bound()
                )
            };

            let low = match range.start_bound() {
                Bound::Unbounded => std::$t::MIN,
                Bound::Included(&x) => x,
                Bound::Excluded(&x) => x.checked_add(1).unwrap_or_else(panic_empty_range),
            };

            let high = match range.end_bound() {
                Bound::Unbounded => std::$t::MAX,
                Bound::Included(&x) => x,
                Bound::Excluded(&x) => x.checked_sub(1).unwrap_or_else(panic_empty_range),
            };

            if low > high {
                panic_empty_range();
            }

            if low == std::$t::MIN && high == std::$t::MAX {
                self.$gen() as $t
            } else {
                let len = high.wrapping_sub(low).wrapping_add(1);
                low.wrapping_add(self.$mod(len as _) as $t)
            }
        }
    };
}

impl Rander {
    #[inline]
    pub fn new() -> Rander {
        Rander::with_seed(RH.try_with(|rh| rh.u64(..)).unwrap_or(0x4d595df4d0f33173))
    }

    #[inline]
    pub fn with_seed(seed: u64) -> Self {
        let rh = Rander(Cell::new(0));

        rh.seed(seed);
        rh
    }

    #[inline]
    pub fn alphabetic(&self) -> char {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let len = CHARS.len() as u8;
        let i = self.u8(..len);
        CHARS[i as usize] as char
    }

    #[inline]
    pub fn alphanumeric(&self) -> char {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let len = CHARS.len() as u8;
        let i = self.u8(..len);
        CHARS[i as usize] as char
    }

    #[inline]
    pub fn bool(&self) -> bool {
        self.u8(..) % 2 == 0
    }

    #[inline]
    pub fn digit(&self, base: u32) -> char {
        if base == 0 {
            panic!("base cannot be zero");
        }
        if base > 36 {
            panic!("base cannot be larger than 36");
        }
        let num = self.u8(..base as u8);
        if num < 10 {
            (b'0' + num) as char
        } else {
            (b'a' + num - 10) as char
        }
    }

    pub fn f32(&self) -> f32 {
        let b = 32;
        let f = std::f32::MANTISSA_DIGITS - 1;
        f32::from_bits((1 << (b - 2)) - (1 << f) + (self.u32(..) >> (b - f))) - 1.0
    }

    pub fn f64(&self) -> f64 {
        let b = 64;
        let f = std::f64::MANTISSA_DIGITS - 1;
        f64::from_bits((1 << (b - 2)) - (1 << f) + (self.u64(..) >> (b - f))) - 1.0
    }

    rand_integer!(
        i8,
        gen_u32,
        gen_mod_u32,
        "Generates a random `i8` in the given range."
    );

    rand_integer!(
        i16,
        gen_u32,
        gen_mod_u32,
        "Generates a random `i16` in the given range."
    );

    rand_integer!(
        i32,
        gen_u32,
        gen_mod_u32,
        "Generates a random `i32` in the given range."
    );

    rand_integer!(
        i64,
        gen_u64,
        gen_mod_u64,
        "Generates a random `i64` in the given range."
    );

    rand_integer!(
        i128,
        gen_u128,
        gen_mod_u128,
        "Generates a random `i128` in the given range."
    );

    #[cfg(target_pointer_width = "16")]
    rand_integer!(
        isize,
        gen_u32,
        gen_mod_u32,
        "Generates a random `isize` in the given range."
    );
    #[cfg(target_pointer_width = "32")]
    rand_integer!(
        isize,
        gen_u32,
        gen_mod_u32,
        "Generates a random `isize` in the given range."
    );
    #[cfg(target_pointer_width = "64")]
    rand_integer!(
        isize,
        gen_u64,
        gen_mod_u64,
        "Generates a random `isize` in the given range."
    );

    #[inline]
    pub fn lowercase(&self) -> char {
        const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
        let len = CHARS.len() as u8;
        let i = self.u8(..len);
        CHARS[i as usize] as char
    }

    #[inline]
    pub fn seed(&self, seed: u64) {
        self.0.set(seed.wrapping_add(1442695040888963407));
        self.gen_u32();
    }

    #[inline]
    pub fn shuffle<T>(&self, slice: &mut [T]) {
        for i in 1..slice.len() {
            slice.swap(i, self.usize(..=i));
        }
    }

    rand_integer!(
        u8,
        gen_u32,
        gen_mod_u32,
        "Generates a random `u8` in the given range."
    );

    rand_integer!(
        u16,
        gen_u32,
        gen_mod_u32,
        "Generates a random `u16` in the given range."
    );

    rand_integer!(
        u32,
        gen_u32,
        gen_mod_u32,
        "Generates a random `u32` in the given range."
    );

    rand_integer!(
        u64,
        gen_u64,
        gen_mod_u64,
        "Generates a random `u64` in the given range."
    );

    rand_integer!(
        u128,
        gen_u128,
        gen_mod_u128,
        "Generates a random `u128` in the given range."
    );

    #[cfg(target_pointer_width = "16")]
    rand_integer!(
        usize,
        gen_u32,
        gen_mod_u32,
        "Generates a random `usize` in the given range."
    );
    #[cfg(target_pointer_width = "32")]
    rand_integer!(
        usize,
        gen_u32,
        gen_mod_u32,
        "Generates a random `usize` in the given range."
    );
    #[cfg(target_pointer_width = "64")]
    rand_integer!(
        usize,
        gen_u64,
        gen_mod_u64,
        "Generates a random `usize` in the given range."
    );
    #[cfg(target_pointer_width = "128")]
    rand_integer!(
        usize,
        gen_u128,
        gen_mod_u128,
        "Generates a random `usize` in the given range."
    );

    #[inline]
    pub fn uppercase(&self) -> char {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let len = CHARS.len() as u8;
        let i = self.u8(..len);
        CHARS[i as usize] as char
    }
}

#[inline]
pub fn seed(seed: u64) {
    RH.with(|rh| rh.seed(seed))
}

#[inline]
pub fn bool() -> bool {
    RH.with(|rh| rh.bool())
}

#[inline]
pub fn alphabetic() -> char {
    RH.with(|rh| rh.alphabetic())
}

#[inline]
pub fn alphanumeric() -> char {
    RH.with(|rh| rh.alphanumeric())
}

#[inline]
pub fn lowercase() -> char {
    RH.with(|rh| rh.lowercase())
}

#[inline]
pub fn uppercase() -> char {
    RH.with(|rh| rh.uppercase())
}

#[inline]
pub fn digit(base: u32) -> char {
    RH.with(|rh| rh.digit(base))
}

#[inline]
pub fn shuffle<T>(slice: &mut [T]) {
    RH.with(|rh| rh.shuffle(slice))
}

macro_rules! integer {
    ($t:tt, $doc:tt) => {
        #[doc = $doc]
        #[inline]
        pub fn $t(range: impl RangeBounds<$t>) -> $t {
            RH.with(|rh| rh.$t(range))
        }
    };
}

integer!(u8, "Generates a random `u8` in the given range.");
integer!(i8, "Generates a random `i8` in the given range.");
integer!(u16, "Generates a random `u16` in the given range.");
integer!(i16, "Generates a random `i16` in the given range.");
integer!(u32, "Generates a random `u32` in the given range.");
integer!(i32, "Generates a random `i32` in the given range.");
integer!(u64, "Generates a random `u64` in the given range.");
integer!(i64, "Generates a random `i64` in the given range.");
integer!(u128, "Generates a random `u128` in the given range.");
integer!(i128, "Generates a random `i128` in the given range.");
integer!(usize, "Generates a random `usize` in the given range.");
integer!(isize, "Generates a random `isize` in the given range.");

pub fn f32() -> f32 {
    RH.with(|rh| rh.f32())
}

pub fn f64() -> f64 {
    RH.with(|rh| rh.f64())
}
