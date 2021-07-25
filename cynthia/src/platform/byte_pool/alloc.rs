pub trait Allocate {
    fn capacity(&self) -> usize;
    fn alloc(size: usize) -> Self;
}

impl<T: Default + Clone> Allocate for Vec<T> {
    fn capacity(&self) -> usize {
        self.len()
    }

    fn alloc(size: usize) -> Self {
        vec![T::default(); size]
    }
}
pub trait Realloc {
    fn realloc(&mut self, new_size: usize);
}

impl<T: Default + Clone> Realloc for Vec<T> {
    fn realloc(&mut self, new_size: usize) {
        use std::cmp::Ordering::*;

        assert!(new_size > 0);
        match new_size.cmp(&self.capacity()) {
            Greater => self.resize(new_size, T::default()),
            Less => {
                self.truncate(new_size);
                self.shrink_to_fit();
            }
            Equal => {}
        }
    }
}
