use std::task::Waker;

#[derive(Debug)]
pub struct Sleeper {
    count: usize,
    wakers: Vec<(usize, Waker)>,
    free_ids: Vec<usize>,
}

impl Sleeper {
    pub fn new() -> Self {
        Sleeper {
            count: 0,
            wakers: Vec::new(),
            free_ids: Vec::new(),
        }
    }

    pub fn insert(&mut self, waker: &Waker) -> usize {
        let id = match self.free_ids.pop() {
            Some(id) => id,
            None => self.count + 1,
        };
        self.count += 1;
        self.wakers.push((id, waker.clone()));
        id
    }

    pub fn update(&mut self, id: usize, waker: &Waker) -> bool {
        for item in &mut self.wakers {
            if item.0 == id {
                if !item.1.will_wake(waker) {
                    item.1 = waker.clone();
                }
                return false;
            }
        }

        self.wakers.push((id, waker.clone()));
        true
    }

    pub fn remove(&mut self, id: usize) -> bool {
        self.count -= 1;
        self.free_ids.push(id);

        for i in (0..self.wakers.len()).rev() {
            if self.wakers[i].0 == id {
                self.wakers.remove(i);
                return false;
            }
        }
        true
    }

    pub fn is_notified(&self) -> bool {
        self.count == 0 || self.count > self.wakers.len()
    }

    pub fn notify(&mut self) -> Option<Waker> {
        if self.wakers.len() == self.count {
            self.wakers.pop().map(|item| item.1)
        } else {
            None
        }
    }
}
