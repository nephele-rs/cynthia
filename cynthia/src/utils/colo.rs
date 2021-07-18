#![forbid(unsafe_code)]

use std::ops::{Index, IndexMut};
use std::{fmt, iter, mem, slice, vec};

#[derive(Clone)]
enum Slot<T> {
    Vacant(usize),
    Occupied(T),
}

impl<T> Slot<T> {
    fn is_occupied(&self) -> bool {
        match self {
            Slot::Vacant(_) => false,
            Slot::Occupied(_) => true,
        }
    }
}

pub struct Colo<T> {
    slots: Vec<Slot<T>>,
    limited: usize,
    len: usize,
    head: usize,
}

impl<T> Colo<T> {
    #[inline]
    pub fn new() -> Self {
        Colo {
            slots: Vec::new(),
            len: 0,
            head: !0,
            limited: 0,
        }
    }

    #[inline]
    pub fn with_capacity(cap: usize) -> Self {
        Colo {
            slots: Vec::with_capacity(cap),
            len: 0,
            head: !0,
            limited: 0,
        }
    }

    #[inline]
    pub fn with_limited(limit: usize) -> Self {
        Colo {
            slots: Vec::new(),
            len: 0,
            head: !0,
            limited: limit,
        }
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.slots.capacity()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn next_vacant(&self) -> usize {
        if self.head == !0 {
            self.len
        } else {
            self.head
        }
    }

    #[inline]
    pub fn insert(&mut self, object: T) -> usize {
        if self.limited != 0 {
            if self.len == self.limited {
                return usize::MIN;
            }
        }
        self.len += 1;
        if self.head == !0 {
            self.slots.push(Slot::Occupied(object));
            self.len - 1
        } else {
            let index = self.head;
            match self.slots[index] {
                Slot::Vacant(next) => {
                    self.head = next;
                    self.slots[index] = Slot::Occupied(object);
                }
                Slot::Occupied(_) => unreachable!(),
            }
            index
        }
    }

    #[inline]
    pub fn remove(&mut self, index: usize) -> Option<T> {
        match self.slots.get_mut(index) {
            None => None,
            Some(&mut Slot::Vacant(_)) => None,
            Some(slot @ &mut Slot::Occupied(_)) => {
                if let Slot::Occupied(object) = mem::replace(slot, Slot::Vacant(self.head)) {
                    self.head = index;
                    self.len -= 1;
                    Some(object)
                } else {
                    unreachable!();
                }
            }
        }
    }

    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(usize, &mut T) -> bool,
    {
        for i in 0..self.slots.len() {
            if let Slot::Occupied(v) = &mut self.slots[i] {
                if !f(i, v) {
                    self.remove(i);
                }
            }
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.slots.clear();
        self.len = 0;
        self.head = !0;
    }

    #[inline]
    pub fn get(&self, index: usize) -> Option<&T> {
        match self.slots.get(index) {
            None => None,
            Some(&Slot::Vacant(_)) => None,
            Some(&Slot::Occupied(ref object)) => Some(object),
        }
    }

    #[inline]
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        match self.slots.get_mut(index) {
            None => None,
            Some(&mut Slot::Vacant(_)) => None,
            Some(&mut Slot::Occupied(ref mut object)) => Some(object),
        }
    }

    #[inline]
    pub fn swap(&mut self, a: usize, b: usize) {
        assert!(self.slots[a].is_occupied(), "invalid object");
        assert!(self.slots[b].is_occupied(), "invalid object");

        if a != b {
            let (a, b) = (a.min(b), a.max(b));
            let (l, r) = self.slots.split_at_mut(b);
            mem::swap(&mut l[a], &mut r[0]);
        }
    }

    pub fn reserve(&mut self, additional: usize) {
        let vacant = self.slots.len() - self.len;
        if additional > vacant {
            self.slots.reserve(additional - vacant);
        }
    }

    pub fn reserve_exact(&mut self, additional: usize) {
        let vacant = self.slots.len() - self.len;
        if additional > vacant {
            self.slots.reserve_exact(additional - vacant);
        }
    }

    #[inline]
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            slots: self.slots.iter().enumerate(),
        }
    }

    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            slots: self.slots.iter_mut().enumerate(),
        }
    }

    pub fn shrink_to_fit(&mut self) {
        self.slots.shrink_to_fit();
    }
}

impl<T> fmt::Debug for Colo<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Colo {{ ... }}")
    }
}

impl<T> Index<usize> for Colo<T> {
    type Output = T;

    #[inline]
    fn index(&self, index: usize) -> &T {
        self.get(index).expect("vacant slot at `index`")
    }
}

impl<T> IndexMut<usize> for Colo<T> {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut T {
        self.get_mut(index).expect("vacant slot at `index`")
    }
}

impl<T> Default for Colo<T> {
    fn default() -> Self {
        Colo::new()
    }
}

impl<T: Clone> Clone for Colo<T> {
    fn clone(&self) -> Self {
        Colo {
            slots: self.slots.clone(),
            len: self.len,
            head: self.head,
            limited: self.limited,
        }
    }
}

pub struct IntoIter<T> {
    slots: iter::Enumerate<vec::IntoIter<Slot<T>>>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = (usize, T);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while let Some((index, slot)) = self.slots.next() {
            if let Slot::Occupied(object) = slot {
                return Some((index, object));
            }
        }
        None
    }
}

impl<T> IntoIterator for Colo<T> {
    type Item = (usize, T);
    type IntoIter = IntoIter<T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            slots: self.slots.into_iter().enumerate(),
        }
    }
}

impl<T> iter::FromIterator<T> for Colo<T> {
    fn from_iter<U: IntoIterator<Item = T>>(iter: U) -> Colo<T> {
        let iter = iter.into_iter();
        let mut colo = Colo::with_capacity(iter.size_hint().0);
        for i in iter {
            colo.insert(i);
        }
        colo
    }
}

impl<T> fmt::Debug for IntoIter<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IntoIter {{ ... }}")
    }
}

pub struct Iter<'a, T> {
    slots: iter::Enumerate<slice::Iter<'a, Slot<T>>>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = (usize, &'a T);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while let Some((index, slot)) = self.slots.next() {
            if let Slot::Occupied(ref object) = *slot {
                return Some((index, object));
            }
        }
        None
    }
}

impl<'a, T> IntoIterator for &'a Colo<T> {
    type Item = (usize, &'a T);
    type IntoIter = Iter<'a, T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> fmt::Debug for Iter<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Iter {{ ... }}")
    }
}

pub struct IterMut<'a, T> {
    slots: iter::Enumerate<slice::IterMut<'a, Slot<T>>>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = (usize, &'a mut T);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while let Some((index, slot)) = self.slots.next() {
            if let Slot::Occupied(ref mut object) = *slot {
                return Some((index, object));
            }
        }
        None
    }
}

impl<'a, T> IntoIterator for &'a mut Colo<T> {
    type Item = (usize, &'a mut T);
    type IntoIter = IterMut<'a, T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<'a, T> fmt::Debug for IterMut<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IterMut {{ ... }}")
    }
}
