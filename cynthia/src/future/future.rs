#[cfg(feature = "alloc")]
extern crate alloc;

use core::fmt;
pub use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use pin_project_lite::pin_project;

#[cfg(feature = "std")]
use std::{
    any::Any,
    panic::{catch_unwind, AssertUnwindSafe, UnwindSafe},
};

#[cfg(feature = "alloc")]
use alloc::boxed::Box;
use core::task::{Context, Poll};

pub fn outstanding<T>() -> Outstanding<T> {
    Outstanding { data: None }
}

#[must_use = "do nothing until you `.await` or poll them"]
pub struct Outstanding<T> {
    data: Option<T>,
}

impl<T> Unpin for Outstanding<T> {}

impl<T> fmt::Debug for Outstanding<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Outstanding").finish()
    }
}

impl<T> Future for Outstanding<T> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if let Some(_) = &self.data {
            return Poll::Ready(());
        }
        Poll::Pending
    }
}

#[cfg(feature = "std")]
pub fn race<T, F1, F2>(future1: F1, future2: F2) -> Race<F1, F2>
where
    F1: Future<Output = T>,
    F2: Future<Output = T>,
{
    Race { future1, future2 }
}

#[cfg(feature = "std")]
pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct Race<F1, F2> {
        #[pin]
        future1: F1,
        #[pin]
        future2: F2,
    }
}

#[cfg(feature = "std")]
impl<T, F1, F2> Future for Race<F1, F2>
where
    F1: Future<Output = T>,
    F2: Future<Output = T>,
{
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        use crate::utils::rander;

        let this = self.project();

        if rander::bool() {
            if let Poll::Ready(t) = this.future1.poll(cx) {
                return Poll::Ready(t);
            }
            if let Poll::Ready(t) = this.future2.poll(cx) {
                return Poll::Ready(t);
            }
        } else {
            if let Poll::Ready(t) = this.future2.poll(cx) {
                return Poll::Ready(t);
            }
            if let Poll::Ready(t) = this.future1.poll(cx) {
                return Poll::Ready(t);
            }
        }
        Poll::Pending
    }
}

pub fn pending<T>() -> Pending<T> {
    Pending {
        _marker: PhantomData,
    }
}

#[must_use = "do nothing until you `.await` or poll them"]
pub struct Pending<T> {
    _marker: PhantomData<T>,
}

impl<T> Unpin for Pending<T> {}

impl<T> fmt::Debug for Pending<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pending").finish()
    }
}

impl<T> Future for Pending<T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<T> {
        Poll::Pending
    }
}

pub fn poll_once<T, F>(f: F) -> PollOnce<F>
where
    F: Future<Output = T>,
{
    PollOnce { f }
}

pin_project! {
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct PollOnce<F> {
        #[pin]
        f: F,
    }
}

impl<F> fmt::Debug for PollOnce<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PollOnce").finish()
    }
}

impl<T, F> Future for PollOnce<F>
where
    F: Future<Output = T>,
{
    type Output = Option<T>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().f.poll(cx) {
            Poll::Ready(t) => Poll::Ready(Some(t)),
            Poll::Pending => Poll::Ready(None),
        }
    }
}

pub fn poll_fn<T, F>(f: F) -> PollFn<F>
where
    F: FnMut(&mut Context<'_>) -> Poll<T>,
{
    PollFn { f }
}

pin_project! {
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct PollFn<F> {
        f: F,
    }
}

impl<F> fmt::Debug for PollFn<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PollFn").finish()
    }
}

impl<T, F> Future for PollFn<F>
where
    F: FnMut(&mut Context<'_>) -> Poll<T>,
{
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let this = self.project();
        (this.f)(cx)
    }
}

pub fn ready<T>(val: T) -> Ready<T> {
    Ready(Some(val))
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct Ready<T>(Option<T>);

impl<T> Unpin for Ready<T> {}

impl<T> Future for Ready<T> {
    type Output = T;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<T> {
        Poll::Ready(self.0.take().expect("`Ready` polled after completion"))
    }
}

pub fn yield_now() -> YieldNow {
    YieldNow(false)
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.0 {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

pub fn zip<F1, F2>(future1: F1, future2: F2) -> Zip<F1, F2>
where
    F1: Future,
    F2: Future,
{
    Zip {
        future1: future1,
        output1: None,
        future2: future2,
        output2: None,
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct Zip<F1, F2>
    where
        F1: Future,
        F2: Future,
    {
        #[pin]
        future1: F1,
        output1: Option<F1::Output>,
        #[pin]
        future2: F2,
        output2: Option<F2::Output>,
    }
}

impl<F1, F2> Future for Zip<F1, F2>
where
    F1: Future,
    F2: Future,
{
    type Output = (F1::Output, F2::Output);
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if this.output1.is_none() {
            if let Poll::Ready(out) = this.future1.poll(cx) {
                *this.output1 = Some(out);
            }
        }

        if this.output2.is_none() {
            if let Poll::Ready(out) = this.future2.poll(cx) {
                *this.output2 = Some(out);
            }
        }

        if this.output1.is_some() && this.output2.is_some() {
            Poll::Ready((this.output1.take().unwrap(), this.output2.take().unwrap()))
        } else {
            Poll::Pending
        }
    }
}

pub fn try_zip<T1, T2, E, F1, F2>(future1: F1, future2: F2) -> TryZip<F1, F2>
where
    F1: Future<Output = Result<T1, E>>,
    F2: Future<Output = Result<T2, E>>,
{
    TryZip {
        future1: future1,
        output1: None,
        future2: future2,
        output2: None,
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct TryZip<F1, F2>
    where
        F1: Future,
        F2: Future,
    {
        #[pin]
        future1: F1,
        output1: Option<F1::Output>,
        #[pin]
        future2: F2,
        output2: Option<F2::Output>,
    }
}

impl<T1, T2, E, F1, F2> Future for TryZip<F1, F2>
where
    F1: Future<Output = Result<T1, E>>,
    F2: Future<Output = Result<T2, E>>,
{
    type Output = Result<(T1, T2), E>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if this.output1.is_none() {
            if let Poll::Ready(out) = this.future1.poll(cx) {
                match out {
                    Ok(t) => *this.output1 = Some(Ok(t)),
                    Err(err) => return Poll::Ready(Err(err)),
                }
            }
        }

        if this.output2.is_none() {
            if let Poll::Ready(out) = this.future2.poll(cx) {
                match out {
                    Ok(t) => *this.output2 = Some(Ok(t)),
                    Err(err) => return Poll::Ready(Err(err)),
                }
            }
        }

        if this.output1.is_some() && this.output2.is_some() {
            let res1 = this.output1.take().unwrap();
            let res2 = this.output2.take().unwrap();
            let t1 = res1.map_err(|_| unreachable!()).unwrap();
            let t2 = res2.map_err(|_| unreachable!()).unwrap();
            Poll::Ready(Ok((t1, t2)))
        } else {
            Poll::Pending
        }
    }
}

pub fn or<T, F1, F2>(future1: F1, future2: F2) -> Or<F1, F2>
where
    F1: Future<Output = T>,
    F2: Future<Output = T>,
{
    Or { future1, future2 }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct Or<F1, F2> {
        #[pin]
        future1: F1,
        #[pin]
        future2: F2,
    }
}

impl<T, F1, F2> Future for Or<F1, F2>
where
    F1: Future<Output = T>,
    F2: Future<Output = T>,
{
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        if let Poll::Ready(t) = this.future1.poll(cx) {
            return Poll::Ready(t);
        }
        if let Poll::Ready(t) = this.future2.poll(cx) {
            return Poll::Ready(t);
        }
        Poll::Pending
    }
}

#[cfg(feature = "std")]
pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct CatchUnwind<F> {
        #[pin]
        inner: F,
    }
}

#[cfg(feature = "std")]
impl<F: Future + UnwindSafe> Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn Any + Send>>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        catch_unwind(AssertUnwindSafe(|| this.inner.poll(cx)))?.map(Ok)
    }
}

#[cfg(feature = "alloc")]
pub type Boxed<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[cfg(feature = "alloc")]
pub type BoxedLocal<T> = Pin<Box<dyn Future<Output = T> + 'static>>;

pub trait FutureExt: Future {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Self::Output>
    where
        Self: Unpin,
    {
        Future::poll(Pin::new(self), cx)
    }

    fn or<F>(self, other: F) -> Or<Self, F>
    where
        Self: Sized,
        F: Future<Output = Self::Output>,
    {
        Or {
            future1: self,
            future2: other,
        }
    }

    #[cfg(feature = "std")]
    fn race<F>(self, other: F) -> Race<Self, F>
    where
        Self: Sized,
        F: Future<Output = Self::Output>,
    {
        Race {
            future1: self,
            future2: other,
        }
    }

    #[cfg(feature = "std")]
    fn catch_unwind(self) -> CatchUnwind<Self>
    where
        Self: Sized + UnwindSafe,
    {
        CatchUnwind { inner: self }
    }

    #[cfg(feature = "alloc")]
    fn boxed<'a>(self) -> Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>
    where
        Self: Sized + Send + 'a,
    {
        Box::pin(self)
    }

    #[cfg(feature = "alloc")]
    fn boxed_local<'a>(self) -> Pin<Box<dyn Future<Output = Self::Output> + 'a>>
    where
        Self: Sized + 'a,
    {
        Box::pin(self)
    }
}

impl<F: Future + ?Sized> FutureExt for F {}
