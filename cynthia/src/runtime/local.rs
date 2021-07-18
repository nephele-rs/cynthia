use std::future::Future;
use std::marker::PhantomData;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::rc::Rc;

use crate::runtime::executor::Executor;
use crate::runtime::task::Runnable;
pub use crate::runtime::task::{self, Task};

struct CallOnDrop<F: Fn()>(F);

impl<F: Fn()> Drop for CallOnDrop<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

#[derive(Debug)]
pub struct LocalExecutor<'a> {
    inner: once_cell::unsync::OnceCell<Executor<'a>>,
    _marker: PhantomData<Rc<()>>,
}

impl UnwindSafe for LocalExecutor<'_> {}
impl RefUnwindSafe for LocalExecutor<'_> {}

impl<'a> LocalExecutor<'a> {
    pub const fn new() -> LocalExecutor<'a> {
        LocalExecutor {
            inner: once_cell::unsync::OnceCell::new(),
            _marker: PhantomData,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner().is_empty()
    }

    pub fn spawn<T: 'a>(&self, future: impl Future<Output = T> + 'a) -> Task<T> {
        let mut active = self.inner().state().active.lock().unwrap();

        let index = active.next_vacant();
        let state = self.inner().state().clone();
        let future = async move {
            let _guard = CallOnDrop(move || drop(state.active.lock().unwrap().remove(index)));
            future.await
        };

        let (runnable, task) =
            unsafe { crate::runtime::task::spawn_unchecked(future, self.schedule()) };
        active.insert(runnable.waker());

        runnable.schedule();
        task
    }

    pub fn try_tick(&self) -> bool {
        self.inner().try_tick()
    }

    pub async fn tick(&self) {
        self.inner().tick().await
    }

    pub async fn run<T>(&self, future: impl Future<Output = T>) -> T {
        self.inner().run(future).await
    }

    fn schedule(&self) -> impl Fn(Runnable) + Send + Sync + 'static {
        let state = self.inner().state().clone();

        move |runnable| {
            state.queue.push(runnable).unwrap();
            state.notify();
        }
    }

    fn inner(&self) -> &Executor<'a> {
        self.inner.get_or_init(|| Executor::new())
    }
}

impl<'a> Default for LocalExecutor<'a> {
    fn default() -> LocalExecutor<'a> {
        LocalExecutor::new()
    }
}
