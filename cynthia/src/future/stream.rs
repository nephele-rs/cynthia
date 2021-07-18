#[cfg(feature = "alloc")]
extern crate alloc;

pub use futures_core::stream::Stream;

#[cfg(feature = "alloc")]
use alloc::boxed::Box;

use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::{fmt, mem};
use pin_project_lite::pin_project;

use crate::ready;

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Enumerate<S> {
        #[pin]
        stream: S,
        i: usize,
    }
}

impl<S> Stream for Enumerate<S>
where
    S: Stream,
{
    type Item = (usize, S::Item);

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        match ready!(this.stream.poll_next(cx)) {
            Some(v) => {
                let ret = (*this.i, v);
                *this.i += 1;
                Poll::Ready(Some(ret))
            }
            None => Poll::Ready(None),
        }
    }
}

pub fn repeat<T: Clone>(item: T) -> Repeat<T> {
    Repeat { item }
}

#[derive(Clone, Debug)]
#[must_use = "do nothing until polled"]
pub struct Repeat<T> {
    item: T,
}

impl<T> Unpin for Repeat<T> {}

impl<T: Clone> Stream for Repeat<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(Some(self.item.clone()))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (usize::max_value(), None)
    }
}

pub fn repeat_with<T, F>(repeater: F) -> RepeatWith<F>
where
    F: FnMut() -> T,
{
    RepeatWith { f: repeater }
}

#[derive(Clone, Debug)]
#[must_use = "do nothing until polled"]
pub struct RepeatWith<F> {
    f: F,
}

impl<F> Unpin for RepeatWith<F> {}

impl<T, F> Stream for RepeatWith<F>
where
    F: FnMut() -> T,
{
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let item = (&mut self.f)();
        Poll::Ready(Some(item))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (usize::max_value(), None)
    }
}

#[cfg(feature = "std")]
pub fn block_on<S: Stream + Unpin>(stream: S) -> BlockOn<S> {
    BlockOn(stream)
}

#[derive(Debug)]
pub struct BlockOn<S>(S);

#[cfg(feature = "std")]
impl<S: Stream + Unpin> Iterator for BlockOn<S> {
    type Item = S::Item;

    fn next(&mut self) -> Option<Self::Item> {
        crate::future::block_on(self.0.next())
    }
}

pub fn empty<T>() -> Empty<T> {
    Empty {
        _marker: PhantomData,
    }
}

#[derive(Clone, Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct Empty<T> {
    _marker: PhantomData<T>,
}

impl<T> Unpin for Empty<T> {}

impl<T> Stream for Empty<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(None)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}

pub fn iter<I: IntoIterator>(iter: I) -> Iter<I::IntoIter> {
    Iter {
        iter: iter.into_iter(),
    }
}

#[derive(Clone, Debug)]
#[must_use = "do nothing until polled"]
pub struct Iter<I> {
    iter: I,
}

impl<I> Unpin for Iter<I> {}

impl<I: Iterator> Stream for Iter<I> {
    type Item = I::Item;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.iter.next())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

pub fn once<T>(t: T) -> Once<T> {
    Once { value: Some(t) }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Once<T> {
        value: Option<T>,
    }
}

impl<T> Stream for Once<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<T>> {
        Poll::Ready(self.project().value.take())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.value.is_some() {
            (1, Some(1))
        } else {
            (0, Some(0))
        }
    }
}

pub fn pending<T>() -> Pending<T> {
    Pending {
        _marker: PhantomData,
    }
}

#[derive(Clone, Debug)]
#[must_use = "do nothing until polled"]
pub struct Pending<T> {
    _marker: PhantomData<T>,
}

impl<T> Unpin for Pending<T> {}

impl<T> Stream for Pending<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<T>> {
        Poll::Pending
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}

pub fn poll_fn<T, F>(f: F) -> PollFn<F>
where
    F: FnMut(&mut Context<'_>) -> Poll<Option<T>>,
{
    PollFn { f }
}

#[derive(Clone)]
#[must_use = "do nothing until polled"]
pub struct PollFn<F> {
    f: F,
}

impl<F> Unpin for PollFn<F> {}

impl<F> fmt::Debug for PollFn<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PollFn").finish()
    }
}

impl<T, F> Stream for PollFn<F>
where
    F: FnMut(&mut Context<'_>) -> Poll<Option<T>>,
{
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        (&mut self.f)(cx)
    }
}

pub fn unfold<T, F, Fut, Item>(seed: T, f: F) -> Unfold<T, F, Fut>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Option<(Item, T)>>,
{
    Unfold {
        f,
        state: Some(seed),
        fut: None,
    }
}

pin_project! {
    #[derive(Clone)]
    #[must_use = "do nothing until polled"]
    pub struct Unfold<T, F, Fut> {
        f: F,
        state: Option<T>,
        #[pin]
        fut: Option<Fut>,
    }
}

impl<T, F, Fut> fmt::Debug for Unfold<T, F, Fut>
where
    T: fmt::Debug,
    Fut: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Unfold")
            .field("state", &self.state)
            .field("fut", &self.fut)
            .finish()
    }
}

impl<T, F, Fut, Item> Stream for Unfold<T, F, Fut>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Option<(Item, T)>>,
{
    type Item = Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        if let Some(state) = this.state.take() {
            this.fut.set(Some((this.f)(state)));
        }

        let step = ready!(this
            .fut
            .as_mut()
            .as_pin_mut()
            .expect("`Unfold` must not be polled after it returned `Poll::Ready(None)`")
            .poll(cx));
        this.fut.set(None);

        if let Some((item, next_state)) = step {
            *this.state = Some(next_state);
            Poll::Ready(Some(item))
        } else {
            Poll::Ready(None)
        }
    }
}

pub fn try_unfold<T, E, F, Fut, Item>(init: T, f: F) -> TryUnfold<T, F, Fut>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Result<Option<(Item, T)>, E>>,
{
    TryUnfold {
        f,
        state: Some(init),
        fut: None,
    }
}

pin_project! {
    #[derive(Clone)]
    #[must_use = "do nothing until polled"]
    pub struct TryUnfold<T, F, Fut> {
        f: F,
        state: Option<T>,
        #[pin]
        fut: Option<Fut>,
    }
}

impl<T, F, Fut> fmt::Debug for TryUnfold<T, F, Fut>
where
    T: fmt::Debug,
    Fut: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TryUnfold")
            .field("state", &self.state)
            .field("fut", &self.fut)
            .finish()
    }
}

impl<T, E, F, Fut, Item> Stream for TryUnfold<T, F, Fut>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = Result<Option<(Item, T)>, E>>,
{
    type Item = Result<Item, E>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        if let Some(state) = this.state.take() {
            this.fut.set(Some((this.f)(state)));
        }

        match this.fut.as_mut().as_pin_mut() {
            None => Poll::Ready(None),
            Some(future) => {
                let step = ready!(future.poll(cx));
                this.fut.set(None);

                match step {
                    Ok(Some((item, next_state))) => {
                        *this.state = Some(next_state);
                        Poll::Ready(Some(Ok(item)))
                    }
                    Ok(None) => Poll::Ready(None),
                    Err(e) => Poll::Ready(Some(Err(e))),
                }
            }
        }
    }
}

pub trait StreamExt: Stream {
    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<Self::Item>>
    where
        Self: Unpin,
    {
        Stream::poll_next(Pin::new(self), cx)
    }

    fn next(&mut self) -> NextFuture<'_, Self>
    where
        Self: Unpin,
    {
        NextFuture { stream: self }
    }

    fn try_next<T, E>(&mut self) -> TryNextFuture<'_, Self>
    where
        Self: Stream<Item = Result<T, E>> + Unpin,
    {
        TryNextFuture { stream: self }
    }

    fn count(self) -> CountFuture<Self>
    where
        Self: Sized,
    {
        CountFuture {
            stream: self,
            count: 0,
        }
    }

    fn map<T, F>(self, f: F) -> Map<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item) -> T,
    {
        Map { stream: self, f }
    }

    fn flat_map<U, F>(self, f: F) -> FlatMap<Self, U, F>
    where
        Self: Sized,
        U: Stream,
        F: FnMut(Self::Item) -> U,
    {
        FlatMap {
            stream: self.map(f),
            inner_stream: None,
        }
    }

    fn flatten(self) -> Flatten<Self>
    where
        Self: Sized,
        Self::Item: Stream,
    {
        Flatten {
            stream: self,
            inner_stream: None,
        }
    }

    fn then<F, Fut>(self, f: F) -> Then<Self, F, Fut>
    where
        Self: Sized,
        F: FnMut(Self::Item) -> Fut,
        Fut: Future,
    {
        Then {
            stream: self,
            future: None,
            f,
        }
    }

    fn filter<P>(self, predicate: P) -> Filter<Self, P>
    where
        Self: Sized,
        P: FnMut(&Self::Item) -> bool,
    {
        Filter {
            stream: self,
            predicate,
        }
    }

    fn filter_map<T, F>(self, f: F) -> FilterMap<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item) -> Option<T>,
    {
        FilterMap { stream: self, f }
    }

    fn take(self, n: usize) -> Take<Self>
    where
        Self: Sized,
    {
        Take { stream: self, n }
    }

    fn take_while<P>(self, predicate: P) -> TakeWhile<Self, P>
    where
        Self: Sized,
        P: FnMut(&Self::Item) -> bool,
    {
        TakeWhile {
            stream: self,
            predicate,
        }
    }

    fn skip(self, n: usize) -> Skip<Self>
    where
        Self: Sized,
    {
        Skip { stream: self, n }
    }

    fn skip_while<P>(self, predicate: P) -> SkipWhile<Self, P>
    where
        Self: Sized,
        P: FnMut(&Self::Item) -> bool,
    {
        SkipWhile {
            stream: self,
            predicate: Some(predicate),
        }
    }

    fn step_by(self, step: usize) -> StepBy<Self>
    where
        Self: Sized,
    {
        assert!(step > 0, "`step` must be greater than zero");
        StepBy {
            stream: self,
            step,
            i: 0,
        }
    }

    fn chain<U>(self, other: U) -> Chain<Self, U>
    where
        Self: Sized,
        U: Stream<Item = Self::Item> + Sized,
    {
        Chain {
            first: self.fuse(),
            second: other.fuse(),
        }
    }

    fn cloned<'a, T>(self) -> Cloned<Self>
    where
        Self: Stream<Item = &'a T> + Sized,
        T: Clone + 'a,
    {
        Cloned { stream: self }
    }

    fn copied<'a, T>(self) -> Copied<Self>
    where
        Self: Stream<Item = &'a T> + Sized,
        T: Copy + 'a,
    {
        Copied { stream: self }
    }

    fn collect<C>(self) -> CollectFuture<Self, C>
    where
        Self: Sized,
        C: Default + Extend<Self::Item>,
    {
        CollectFuture {
            stream: self,
            collection: Default::default(),
        }
    }

    fn try_collect<T, E, C>(self) -> TryCollectFuture<Self, C>
    where
        Self: Stream<Item = Result<T, E>> + Sized,
        C: Default + Extend<T>,
    {
        TryCollectFuture {
            stream: self,
            items: Default::default(),
        }
    }

    fn partition<B, P>(self, predicate: P) -> PartitionFuture<Self, P, B>
    where
        Self: Sized,
        B: Default + Extend<Self::Item>,
        P: FnMut(&Self::Item) -> bool,
    {
        PartitionFuture {
            stream: self,
            predicate,
            res: Some(Default::default()),
        }
    }

    fn fold<T, F>(self, init: T, f: F) -> FoldFuture<Self, F, T>
    where
        Self: Sized,
        F: FnMut(T, Self::Item) -> T,
    {
        FoldFuture {
            stream: self,
            f,
            acc: Some(init),
        }
    }

    fn try_fold<T, E, F, B>(&mut self, init: B, f: F) -> TryFoldFuture<'_, Self, F, B>
    where
        Self: Stream<Item = Result<T, E>> + Unpin + Sized,
        F: FnMut(B, T) -> Result<B, E>,
    {
        TryFoldFuture {
            stream: self,
            f,
            acc: Some(init),
        }
    }

    fn scan<St, B, F>(self, initial_state: St, f: F) -> Scan<Self, St, F>
    where
        Self: Sized,
        F: FnMut(&mut St, Self::Item) -> Option<B>,
    {
        Scan {
            stream: self,
            state_f: (initial_state, f),
        }
    }

    fn fuse(self) -> Fuse<Self>
    where
        Self: Sized,
    {
        Fuse {
            stream: self,
            done: false,
        }
    }

    fn cycle(self) -> Cycle<Self>
    where
        Self: Clone + Sized,
    {
        Cycle {
            orig: self.clone(),
            stream: self,
        }
    }

    fn enumerate(self) -> Enumerate<Self>
    where
        Self: Sized,
    {
        Enumerate { stream: self, i: 0 }
    }

    fn inspect<F>(self, f: F) -> Inspect<Self, F>
    where
        Self: Sized,
        F: FnMut(&Self::Item),
    {
        Inspect { stream: self, f }
    }

    fn nth(&mut self, n: usize) -> NthFuture<'_, Self>
    where
        Self: Unpin,
    {
        NthFuture { stream: self, n }
    }

    fn last(self) -> LastFuture<Self>
    where
        Self: Sized,
    {
        LastFuture {
            stream: self,
            last: None,
        }
    }

    fn find<P>(&mut self, predicate: P) -> FindFuture<'_, Self, P>
    where
        Self: Unpin,
        P: FnMut(&Self::Item) -> bool,
    {
        FindFuture {
            stream: self,
            predicate,
        }
    }

    fn find_map<F, B>(&mut self, f: F) -> FindMapFuture<'_, Self, F>
    where
        Self: Unpin,
        F: FnMut(Self::Item) -> Option<B>,
    {
        FindMapFuture { stream: self, f }
    }

    fn position<P>(&mut self, predicate: P) -> PositionFuture<'_, Self, P>
    where
        Self: Unpin,
        P: FnMut(Self::Item) -> bool,
    {
        PositionFuture {
            stream: self,
            predicate,
            index: 0,
        }
    }

    fn all<P>(&mut self, predicate: P) -> AllFuture<'_, Self, P>
    where
        Self: Unpin,
        P: FnMut(Self::Item) -> bool,
    {
        AllFuture {
            stream: self,
            predicate,
        }
    }

    fn any<P>(&mut self, predicate: P) -> AnyFuture<'_, Self, P>
    where
        Self: Unpin,
        P: FnMut(Self::Item) -> bool,
    {
        AnyFuture {
            stream: self,
            predicate,
        }
    }

    fn for_each<F>(self, f: F) -> ForEachFuture<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item),
    {
        ForEachFuture { stream: self, f }
    }

    fn try_for_each<F, E>(&mut self, f: F) -> TryForEachFuture<'_, Self, F>
    where
        Self: Unpin,
        F: FnMut(Self::Item) -> Result<(), E>,
    {
        TryForEachFuture { stream: self, f }
    }

    fn zip<U>(self, other: U) -> Zip<Self, U>
    where
        Self: Sized,
        U: Stream,
    {
        Zip {
            item_slot: None,
            first: self,
            second: other,
        }
    }

    fn unzip<A, B, FromA, FromB>(self) -> UnzipFuture<Self, FromA, FromB>
    where
        FromA: Default + Extend<A>,
        FromB: Default + Extend<B>,
        Self: Stream<Item = (A, B)> + Sized,
    {
        UnzipFuture {
            stream: self,
            res: Some(Default::default()),
        }
    }

    fn or<S>(self, other: S) -> Or<Self, S>
    where
        Self: Sized,
        S: Stream<Item = Self::Item>,
    {
        Or {
            stream1: self,
            stream2: other,
        }
    }

    #[cfg(feature = "std")]
    fn race<S>(self, other: S) -> Race<Self, S>
    where
        Self: Sized,
        S: Stream<Item = Self::Item>,
    {
        Race {
            stream1: self,
            stream2: other,
        }
    }

    #[cfg(feature = "alloc")]
    fn boxed<'a>(self) -> Pin<Box<dyn Stream<Item = Self::Item> + Send + 'a>>
    where
        Self: Send + Sized + 'a,
    {
        Box::pin(self)
    }

    #[cfg(feature = "alloc")]
    fn boxed_local<'a>(self) -> Pin<Box<dyn Stream<Item = Self::Item> + 'a>>
    where
        Self: Sized + 'a,
    {
        Box::pin(self)
    }
}

impl<S: Stream + ?Sized> StreamExt for S {}

#[cfg(feature = "alloc")]
pub type Boxed<T> = Pin<Box<dyn Stream<Item = T> + Send + 'static>>;

#[cfg(feature = "alloc")]
pub type BoxedLocal<T> = Pin<Box<dyn Stream<Item = T> + 'static>>;

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct NextFuture<'a, S: ?Sized> {
    stream: &'a mut S,
}

impl<S: Unpin + ?Sized> Unpin for NextFuture<'_, S> {}

impl<S: Stream + Unpin + ?Sized> Future for NextFuture<'_, S> {
    type Output = Option<S::Item>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.stream.poll_next(cx)
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct TryNextFuture<'a, S: ?Sized> {
    stream: &'a mut S,
}

impl<S: Unpin + ?Sized> Unpin for TryNextFuture<'_, S> {}

impl<T, E, S> Future for TryNextFuture<'_, S>
where
    S: Stream<Item = Result<T, E>> + Unpin + ?Sized,
{
    type Output = Result<Option<T>, E>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let res = ready!(self.stream.poll_next(cx));
        Poll::Ready(res.transpose())
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct CountFuture<S: ?Sized> {
        count: usize,
        #[pin]
        stream: S,
    }
}

impl<S: Stream + ?Sized> Future for CountFuture<S> {
    type Output = usize;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match ready!(self.as_mut().project().stream.poll_next(cx)) {
                None => return Poll::Ready(self.count),
                Some(_) => *self.as_mut().project().count += 1,
            }
        }
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct CollectFuture<S, C> {
        #[pin]
        stream: S,
        collection: C,
    }
}

impl<S, C> Future for CollectFuture<S, C>
where
    S: Stream,
    C: Default + Extend<S::Item>,
{
    type Output = C;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<C> {
        let mut this = self.as_mut().project();
        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some(e) => this.collection.extend(Some(e)),
                None => {
                    return Poll::Ready(mem::replace(self.project().collection, Default::default()))
                }
            }
        }
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct TryCollectFuture<S, C> {
        #[pin]
        stream: S,
        items: C,
    }
}

impl<T, E, S, C> Future for TryCollectFuture<S, C>
where
    S: Stream<Item = Result<T, E>>,
    C: Default + Extend<T>,
{
    type Output = Result<C, E>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        Poll::Ready(Ok(loop {
            match ready!(this.stream.as_mut().poll_next(cx)?) {
                Some(x) => this.items.extend(Some(x)),
                None => break mem::replace(this.items, Default::default()),
            }
        }))
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct PartitionFuture<S, P, B> {
        #[pin]
        stream: S,
        predicate: P,
        res: Option<(B, B)>,
    }
}

impl<S, P, B> Future for PartitionFuture<S, P, B>
where
    S: Stream + Sized,
    P: FnMut(&S::Item) -> bool,
    B: Default + Extend<S::Item>,
{
    type Output = (B, B);
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some(v) => {
                    let res = this.res.as_mut().unwrap();
                    if (this.predicate)(&v) {
                        res.0.extend(Some(v))
                    } else {
                        res.1.extend(Some(v))
                    }
                }
                None => return Poll::Ready(this.res.take().unwrap()),
            }
        }
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct FoldFuture<S, F, T> {
        #[pin]
        stream: S,
        f: F,
        acc: Option<T>,
    }
}

impl<S, F, T> Future for FoldFuture<S, F, T>
where
    S: Stream,
    F: FnMut(T, S::Item) -> T,
{
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some(v) => {
                    let old = this.acc.take().unwrap();
                    let new = (this.f)(old, v);
                    *this.acc = Some(new);
                }
                None => return Poll::Ready(this.acc.take().unwrap()),
            }
        }
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct TryFoldFuture<'a, S, F, B> {
    stream: &'a mut S,
    f: F,
    acc: Option<B>,
}

impl<'a, S, F, B> Unpin for TryFoldFuture<'a, S, F, B> {}

impl<'a, T, E, S, F, B> Future for TryFoldFuture<'a, S, F, B>
where
    S: Stream<Item = Result<T, E>> + Unpin,
    F: FnMut(B, T) -> Result<B, E>,
{
    type Output = Result<B, E>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match ready!(self.stream.poll_next(cx)) {
                Some(Err(e)) => return Poll::Ready(Err(e)),
                Some(Ok(t)) => {
                    let old = self.acc.take().unwrap();
                    let new = (&mut self.f)(old, t);

                    match new {
                        Ok(t) => self.acc = Some(t),
                        Err(e) => return Poll::Ready(Err(e)),
                    }
                }
                None => return Poll::Ready(Ok(self.acc.take().unwrap())),
            }
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Scan<S, St, F> {
        #[pin]
        stream: S,
        state_f: (St, F),
    }
}

impl<S, St, F, B> Stream for Scan<S, St, F>
where
    S: Stream,
    F: FnMut(&mut St, S::Item) -> Option<B>,
{
    type Item = B;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<B>> {
        let mut this = self.project();
        this.stream.as_mut().poll_next(cx).map(|item| {
            item.and_then(|item| {
                let (state, f) = this.state_f;
                f(state, item)
            })
        })
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Fuse<S> {
        #[pin]
        stream: S,
        done: bool,
    }
}

impl<S: Stream> Stream for Fuse<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        let this = self.project();

        if *this.done {
            Poll::Ready(None)
        } else {
            let next = ready!(this.stream.poll_next(cx));
            if next.is_none() {
                *this.done = true;
            }
            Poll::Ready(next)
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Map<S, F> {
        #[pin]
        stream: S,
        f: F,
    }
}

impl<S, F, T> Stream for Map<S, F>
where
    S: Stream,
    F: FnMut(S::Item) -> T,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let next = ready!(this.stream.poll_next(cx));
        Poll::Ready(next.map(this.f))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.stream.size_hint()
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct FlatMap<S, U, F> {
        #[pin]
        stream: Map<S, F>,
        #[pin]
        inner_stream: Option<U>,
    }
}

impl<S, U, F> Stream for FlatMap<S, U, F>
where
    S: Stream,
    U: Stream,
    F: FnMut(S::Item) -> U,
{
    type Item = U::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            if let Some(inner) = this.inner_stream.as_mut().as_pin_mut() {
                match ready!(inner.poll_next(cx)) {
                    Some(item) => return Poll::Ready(Some(item)),
                    None => this.inner_stream.set(None),
                }
            }

            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some(stream) => this.inner_stream.set(Some(stream)),
                None => return Poll::Ready(None),
            }
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Flatten<S: Stream> {
        #[pin]
        stream: S,
        #[pin]
        inner_stream: Option<S::Item>,
    }
}

impl<S, U> Stream for Flatten<S>
where
    S: Stream<Item = U>,
    U: Stream,
{
    type Item = U::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            if let Some(inner) = this.inner_stream.as_mut().as_pin_mut() {
                match ready!(inner.poll_next(cx)) {
                    Some(item) => return Poll::Ready(Some(item)),
                    None => this.inner_stream.set(None),
                }
            }

            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some(inner) => this.inner_stream.set(Some(inner)),
                None => return Poll::Ready(None),
            }
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Then<S, F, Fut> {
        #[pin]
        stream: S,
        #[pin]
        future: Option<Fut>,
        f: F,
    }
}

impl<S, F, Fut> Stream for Then<S, F, Fut>
where
    S: Stream,
    F: FnMut(S::Item) -> Fut,
    Fut: Future,
{
    type Item = Fut::Output;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            if let Some(fut) = this.future.as_mut().as_pin_mut() {
                let item = ready!(fut.poll(cx));
                this.future.set(None);
                return Poll::Ready(Some(item));
            } else if let Some(item) = ready!(this.stream.as_mut().poll_next(cx)) {
                this.future.set(Some((this.f)(item)));
            } else {
                return Poll::Ready(None);
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let future_len = if self.future.is_some() { 1 } else { 0 };
        let (lower, upper) = self.stream.size_hint();
        let lower = lower.saturating_add(future_len);
        let upper = upper.and_then(|u| u.checked_add(future_len));
        (lower, upper)
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Filter<S, P> {
        #[pin]
        stream: S,
        predicate: P,
    }
}

impl<S, P> Stream for Filter<S, P>
where
    S: Stream,
    P: FnMut(&S::Item) -> bool,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                None => return Poll::Ready(None),
                Some(v) if (this.predicate)(&v) => return Poll::Ready(Some(v)),
                Some(_) => {}
            }
        }
    }
}

pub fn or<T, S1, S2>(stream1: S1, stream2: S2) -> Or<S1, S2>
where
    S1: Stream<Item = T>,
    S2: Stream<Item = T>,
{
    Or { stream1, stream2 }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Or<S1, S2> {
        #[pin]
        stream1: S1,
        #[pin]
        stream2: S2,
    }
}

impl<T, S1, S2> Stream for Or<S1, S2>
where
    S1: Stream<Item = T>,
    S2: Stream<Item = T>,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        if let Poll::Ready(Some(t)) = this.stream1.as_mut().poll_next(cx) {
            return Poll::Ready(Some(t));
        }
        this.stream2.as_mut().poll_next(cx)
    }
}

#[cfg(feature = "std")]
pub fn race<T, S1, S2>(stream1: S1, stream2: S2) -> Race<S1, S2>
where
    S1: Stream<Item = T>,
    S2: Stream<Item = T>,
{
    Race { stream1, stream2 }
}

#[cfg(feature = "std")]
pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Race<S1, S2> {
        #[pin]
        stream1: S1,
        #[pin]
        stream2: S2,
    }
}

#[cfg(feature = "std")]
impl<T, S1, S2> Stream for Race<S1, S2>
where
    S1: Stream<Item = T>,
    S2: Stream<Item = T>,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        use crate::utils::rander;

        let mut this = self.project();

        if rander::bool() {
            if let Poll::Ready(Some(t)) = this.stream1.as_mut().poll_next(cx) {
                return Poll::Ready(Some(t));
            }
            if let Poll::Ready(Some(t)) = this.stream2.as_mut().poll_next(cx) {
                return Poll::Ready(Some(t));
            }
        } else {
            if let Poll::Ready(Some(t)) = this.stream2.as_mut().poll_next(cx) {
                return Poll::Ready(Some(t));
            }
            if let Poll::Ready(Some(t)) = this.stream1.as_mut().poll_next(cx) {
                return Poll::Ready(Some(t));
            }
        }
        Poll::Pending
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct FilterMap<S, F> {
        #[pin]
        stream: S,
        f: F,
    }
}

impl<S, F, T> Stream for FilterMap<S, F>
where
    S: Stream,
    F: FnMut(S::Item) -> Option<T>,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                None => return Poll::Ready(None),
                Some(v) => {
                    if let Some(t) = (this.f)(v) {
                        return Poll::Ready(Some(t));
                    }
                }
            }
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Take<S> {
        #[pin]
        stream: S,
        n: usize,
    }
}

impl<S: Stream> Stream for Take<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        let this = self.project();

        if *this.n == 0 {
            Poll::Ready(None)
        } else {
            let next = ready!(this.stream.poll_next(cx));
            match next {
                Some(_) => *this.n -= 1,
                None => *this.n = 0,
            }
            Poll::Ready(next)
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct TakeWhile<S, P> {
        #[pin]
        stream: S,
        predicate: P,
    }
}

impl<S, P> Stream for TakeWhile<S, P>
where
    S: Stream,
    P: FnMut(&S::Item) -> bool,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        match ready!(this.stream.poll_next(cx)) {
            Some(v) => {
                if (this.predicate)(&v) {
                    Poll::Ready(Some(v))
                } else {
                    Poll::Ready(None)
                }
            }
            None => Poll::Ready(None),
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Skip<S> {
        #[pin]
        stream: S,
        n: usize,
    }
}

impl<S: Stream> Stream for Skip<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some(v) => match *this.n {
                    0 => return Poll::Ready(Some(v)),
                    _ => *this.n -= 1,
                },
                None => return Poll::Ready(None),
            }
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct SkipWhile<S, P> {
        #[pin]
        stream: S,
        predicate: Option<P>,
    }
}

impl<S, P> Stream for SkipWhile<S, P>
where
    S: Stream,
    P: FnMut(&S::Item) -> bool,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some(v) => match this.predicate {
                    Some(p) => {
                        if !p(&v) {
                            *this.predicate = None;
                            return Poll::Ready(Some(v));
                        }
                    }
                    None => return Poll::Ready(Some(v)),
                },
                None => return Poll::Ready(None),
            }
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct StepBy<S> {
        #[pin]
        stream: S,
        step: usize,
        i: usize,
    }
}

impl<S: Stream> Stream for StepBy<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some(v) => {
                    if *this.i == 0 {
                        *this.i = *this.step - 1;
                        return Poll::Ready(Some(v));
                    } else {
                        *this.i -= 1;
                    }
                }
                None => return Poll::Ready(None),
            }
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Chain<S, U> {
        #[pin]
        first: Fuse<S>,
        #[pin]
        second: Fuse<U>,
    }
}

impl<S: Stream, U: Stream<Item = S::Item>> Stream for Chain<S, U> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        if !this.first.done {
            let next = ready!(this.first.as_mut().poll_next(cx));
            if let Some(next) = next {
                return Poll::Ready(Some(next));
            }
        }

        if !this.second.done {
            let next = ready!(this.second.as_mut().poll_next(cx));
            if let Some(next) = next {
                return Poll::Ready(Some(next));
            }
        }

        if this.first.done && this.second.done {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Cloned<S> {
        #[pin]
        stream: S,
    }
}

impl<'a, S, T: 'a> Stream for Cloned<S>
where
    S: Stream<Item = &'a T>,
    T: Clone,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let next = ready!(this.stream.poll_next(cx));
        Poll::Ready(next.cloned())
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Copied<S> {
        #[pin]
        stream: S,
    }
}

impl<'a, S, T: 'a> Stream for Copied<S>
where
    S: Stream<Item = &'a T>,
    T: Copy,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let next = ready!(this.stream.poll_next(cx));
        Poll::Ready(next.copied())
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Cycle<S> {
        orig: S,
        #[pin]
        stream: S,
    }
}

impl<S> Stream for Cycle<S>
where
    S: Stream + Clone,
{
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match ready!(self.as_mut().project().stream.as_mut().poll_next(cx)) {
            Some(item) => Poll::Ready(Some(item)),
            None => {
                let new = self.as_mut().orig.clone();
                self.as_mut().project().stream.set(new);
                self.project().stream.poll_next(cx)
            }
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "do nothing until polled"]
    pub struct Inspect<S, F> {
        #[pin]
        stream: S,
        f: F,
    }
}

impl<S, F> Stream for Inspect<S, F>
where
    S: Stream,
    F: FnMut(&S::Item),
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        let next = ready!(this.stream.as_mut().poll_next(cx));
        if let Some(x) = &next {
            (this.f)(x);
        }
        Poll::Ready(next)
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct NthFuture<'a, S: ?Sized> {
    stream: &'a mut S,
    n: usize,
}

impl<S: Unpin + ?Sized> Unpin for NthFuture<'_, S> {}

impl<'a, S> Future for NthFuture<'a, S>
where
    S: Stream + Unpin + ?Sized,
{
    type Output = Option<S::Item>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match ready!(self.stream.poll_next(cx)) {
                Some(v) => match self.n {
                    0 => return Poll::Ready(Some(v)),
                    _ => self.n -= 1,
                },
                None => return Poll::Ready(None),
            }
        }
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct LastFuture<S: Stream> {
        #[pin]
        stream: S,
        last: Option<S::Item>,
    }
}

impl<S: Stream> Future for LastFuture<S> {
    type Output = Option<S::Item>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some(new) => *this.last = Some(new),
                None => return Poll::Ready(this.last.take()),
            }
        }
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct FindFuture<'a, S: ?Sized, P> {
    stream: &'a mut S,
    predicate: P,
}

impl<S: Unpin + ?Sized, P> Unpin for FindFuture<'_, S, P> {}

impl<'a, S, P> Future for FindFuture<'a, S, P>
where
    S: Stream + Unpin + ?Sized,
    P: FnMut(&S::Item) -> bool,
{
    type Output = Option<S::Item>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match ready!(self.stream.poll_next(cx)) {
                Some(v) if (&mut self.predicate)(&v) => return Poll::Ready(Some(v)),
                Some(_) => {}
                None => return Poll::Ready(None),
            }
        }
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct FindMapFuture<'a, S: ?Sized, F> {
    stream: &'a mut S,
    f: F,
}

impl<S: Unpin + ?Sized, F> Unpin for FindMapFuture<'_, S, F> {}

impl<'a, S, B, F> Future for FindMapFuture<'a, S, F>
where
    S: Stream + Unpin + ?Sized,
    F: FnMut(S::Item) -> Option<B>,
{
    type Output = Option<B>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match ready!(self.stream.poll_next(cx)) {
                Some(v) => {
                    if let Some(v) = (&mut self.f)(v) {
                        return Poll::Ready(Some(v));
                    }
                }
                None => return Poll::Ready(None),
            }
        }
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct PositionFuture<'a, S: ?Sized, P> {
    stream: &'a mut S,
    predicate: P,
    index: usize,
}

impl<'a, S: Unpin + ?Sized, P> Unpin for PositionFuture<'a, S, P> {}

impl<'a, S, P> Future for PositionFuture<'a, S, P>
where
    S: Stream + Unpin + ?Sized,
    P: FnMut(S::Item) -> bool,
{
    type Output = Option<usize>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match ready!(self.stream.poll_next(cx)) {
                Some(v) => {
                    if (&mut self.predicate)(v) {
                        return Poll::Ready(Some(self.index));
                    } else {
                        self.index += 1;
                    }
                }
                None => return Poll::Ready(None),
            }
        }
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct AllFuture<'a, S: ?Sized, P> {
    stream: &'a mut S,
    predicate: P,
}

impl<S: Unpin + ?Sized, P> Unpin for AllFuture<'_, S, P> {}

impl<S, P> Future for AllFuture<'_, S, P>
where
    S: Stream + Unpin + ?Sized,
    P: FnMut(S::Item) -> bool,
{
    type Output = bool;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match ready!(self.stream.poll_next(cx)) {
                Some(v) => {
                    if !(&mut self.predicate)(v) {
                        return Poll::Ready(false);
                    }
                }
                None => return Poll::Ready(true),
            }
        }
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct AnyFuture<'a, S: ?Sized, P> {
    stream: &'a mut S,
    predicate: P,
}

impl<S: Unpin + ?Sized, P> Unpin for AnyFuture<'_, S, P> {}

impl<S, P> Future for AnyFuture<'_, S, P>
where
    S: Stream + Unpin + ?Sized,
    P: FnMut(S::Item) -> bool,
{
    type Output = bool;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match ready!(self.stream.poll_next(cx)) {
                Some(v) => {
                    if (&mut self.predicate)(v) {
                        return Poll::Ready(true);
                    }
                }
                None => return Poll::Ready(false),
            }
        }
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct ForEachFuture<S, F> {
        #[pin]
        stream: S,
        f: F,
    }
}

impl<S, F> Future for ForEachFuture<S, F>
where
    S: Stream,
    F: FnMut(S::Item),
{
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some(v) => (this.f)(v),
                None => return Poll::Ready(()),
            }
        }
    }
}

#[derive(Debug)]
#[must_use = "do nothing until you `.await` or poll them"]
pub struct TryForEachFuture<'a, S: ?Sized, F> {
    stream: &'a mut S,
    f: F,
}

impl<'a, S: Unpin + ?Sized, F> Unpin for TryForEachFuture<'a, S, F> {}

impl<'a, S, F, E> Future for TryForEachFuture<'a, S, F>
where
    S: Stream + Unpin + ?Sized,
    F: FnMut(S::Item) -> Result<(), E>,
{
    type Output = Result<(), E>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            match ready!(self.stream.poll_next(cx)) {
                None => return Poll::Ready(Ok(())),
                Some(v) => (&mut self.f)(v)?,
            }
        }
    }
}

pin_project! {
    #[derive(Clone, Debug)]
    #[must_use = "streams do nothing unless polled"]
    pub struct Zip<A: Stream, B> {
        item_slot: Option<A::Item>,
        #[pin]
        first: A,
        #[pin]
        second: B,
    }
}

impl<A: Stream, B: Stream> Stream for Zip<A, B> {
    type Item = (A::Item, B::Item);

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();

        if this.item_slot.is_none() {
            match this.first.poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(item)) => *this.item_slot = Some(item),
            }
        }

        let second_item = ready!(this.second.poll_next(cx));
        let first_item = this.item_slot.take().unwrap();
        Poll::Ready(second_item.map(|second_item| (first_item, second_item)))
    }
}

pin_project! {
    #[derive(Debug)]
    #[must_use = "do nothing until you `.await` or poll them"]
    pub struct UnzipFuture<S, FromA, FromB> {
        #[pin]
        stream: S,
        res: Option<(FromA, FromB)>,
    }
}

impl<S, A, B, FromA, FromB> Future for UnzipFuture<S, FromA, FromB>
where
    S: Stream<Item = (A, B)>,
    FromA: Default + Extend<A>,
    FromB: Default + Extend<B>,
{
    type Output = (FromA, FromB);
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            match ready!(this.stream.as_mut().poll_next(cx)) {
                Some((a, b)) => {
                    let res = this.res.as_mut().unwrap();
                    res.0.extend(Some(a));
                    res.1.extend(Some(b));
                }
                None => return Poll::Ready(this.res.take().unwrap()),
            }
        }
    }
}
