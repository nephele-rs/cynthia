use std::{fmt, mem, panic, process, sync::mpsc, thread};

#[derive(Default)]
#[must_use]
pub struct Parallel<'a, T> {
    closures: Vec<Box<dyn FnOnce() -> T + Send + 'a>>,
}

impl<'a, T> Parallel<'a, T> {
    pub fn new() -> Parallel<'a, T> {
        Parallel {
            closures: Vec::new(),
        }
    }

    pub fn add<F>(mut self, f: F) -> Parallel<'a, T>
    where
        F: FnOnce() -> T + Send + 'a,
        T: Send + 'a,
    {
        self.closures.push(Box::new(f));
        self
    }

    pub fn each<A, I, F>(mut self, iter: I, f: F) -> Parallel<'a, T>
    where
        I: IntoIterator<Item = A>,
        F: FnOnce(A) -> T + Clone + Send + 'a,
        A: Send + 'a,
        T: Send + 'a,
    {
        for t in iter.into_iter() {
            let f = f.clone();
            self.closures.push(Box::new(|| f(t)));
        }
        self
    }

    pub fn run(mut self) -> Vec<T>
    where
        T: Send + 'a,
    {
        let f = match self.closures.pop() {
            None => return Vec::new(),
            Some(f) => f,
        };

        let (mut results, r) = self.finish(f);
        results.push(r);
        results
    }

    pub fn finish<F, R>(self, f: F) -> (Vec<T>, R)
    where
        F: FnOnce() -> R,
        T: Send + 'a,
    {
        let guard = NoPanic;

        let mut handles = Vec::new();

        let mut receivers = Vec::new();

        for f in self.closures.into_iter() {
            let (sender, receiver) = mpsc::channel();
            let f = move || sender.send(f()).unwrap();

            let f: Box<dyn FnOnce() + Send + 'a> = Box::new(f);
            let f: Box<dyn FnOnce() + Send + 'static> = unsafe { mem::transmute(f) };

            handles.push(thread::spawn(f));
            receivers.push(receiver);
        }

        let mut last_err = None;

        let res = panic::catch_unwind(panic::AssertUnwindSafe(f));

        for h in handles {
            if let Err(err) = h.join() {
                last_err = Some(err);
            }
        }

        drop(guard);

        if let Some(err) = last_err {
            panic::resume_unwind(err);
        }

        let mut results = Vec::new();
        for receiver in receivers {
            results.push(receiver.recv().unwrap());
        }

        match res {
            Ok(r) => (results, r),
            Err(err) => panic::resume_unwind(err),
        }
    }
}

impl<T> fmt::Debug for Parallel<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Parallel")
            .field("len", &self.closures.len())
            .finish()
    }
}

struct NoPanic;

impl Drop for NoPanic {
    fn drop(&mut self) {
        if thread::panicking() {
            process::abort();
        }
    }
}
