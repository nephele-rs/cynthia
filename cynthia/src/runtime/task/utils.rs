use core::alloc::Layout;
use core::mem;

pub(crate) fn abort() -> ! {
    struct Panic;

    impl Drop for Panic {
        fn drop(&mut self) {
            panic!("aborting the process");
        }
    }

    let _panic = Panic;
    panic!("aborting");
}

#[inline]
pub(crate) fn abort_on_panic<T>(f: impl FnOnce() -> T) -> T {
    struct Bomb;

    impl Drop for Bomb {
        fn drop(&mut self) {
            abort();
        }
    }

    let bomb = Bomb;
    let t = f();
    mem::forget(bomb);
    t
}

#[inline]
pub(crate) fn extend(a: Layout, b: Layout) -> (Layout, usize) {
    let new_align = a.align().max(b.align());
    let pad = padding_needed_for(a, b.align());

    let offset = a.size().checked_add(pad).unwrap();
    let new_size = offset.checked_add(b.size()).unwrap();

    let layout = Layout::from_size_align(new_size, new_align).unwrap();
    (layout, offset)
}

#[inline]
pub(crate) fn padding_needed_for(layout: Layout, align: usize) -> usize {
    let len = layout.size();
    let len_rounded_up = len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1);
    len_rounded_up.wrapping_sub(len)
}
