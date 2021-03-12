//! ## Introduction
//! This crate implements completion-handler based futures.
//! It's intended for single-threaded asynchronous programming with reactor / proactor pattern,
//! similar to the asynchrony model of *Node.js*.
//! ## Motivation
//! In Rust's native [`future`](std::future), when a sub-future completes,
//! the root task is awakened, which recursively polls children futures until reaching the true point of reentrance.
//! In the future provided by this crate, when a sub-future completes,
//! the next future to run is directly awakened, rather than having to be polled by parent futures.
//! Essentially, each future is its own root task. This resembles Javascript's Promises,
//! where they immediately start execution upon creation. Hence the future type provided by this create is named [`Eager`].
//!
//! Consider the following code using Rust's native future. It simply chains `n` futures together using `then`,
//! where each future performs some I/O operation simulated here by `yield_now`.
//! One would expect the time for running it to be linear to `n`, but actually it's quadratic to `n`
//! because each time the I/O operation completes, we have to poll through the `next` chain from the beginning.
//! The future provided by this crate doesn't exhibit this behavior.
//! ```ignore
//! fn chain_many(n: isize) -> BoxFuture<'static, isize> {
//!     let mut future = async { 0 }.boxed();
//!     for _ in 0..n {
//!         future = future.then(|x| async move {
//!             tokio::task::yield_now().await;
//!             x + 1
//!         }).boxed()
//!     }
//!     future
//! }
//! ```
//! ## Notes
//! Compared to Rust's native futures, this approach introduces dynamic allocation overhead.
//! This crate is suitable only if your control flow is dynamically composed such as in the example above.
//! For control flow that is mostly static, Rust's native future could perform much better
//! thanks to optimizations such as aggressive inlining. This crate doesn't work with `async` / `await`.

use std::cell::RefCell;
use std::rc::Rc;

/// Internal state of [`Eager`].
pub enum State<'a, T> {
    /// Handler hasn't been setup yet. [`Eager`] shouldn't be completed in this state.
    New,
    /// Do nothing when the [`Eager`] completes.
    NoHandler,
    /// Call a [`FnOnce`] when the [`Eager`] completes.
    HasHandler(Box<dyn FnOnce(T) + 'a>),
    /// [`Eager`] has completed and handler (if any) has been called.
    Completed,
}

/// Internal state of [`Eager`] along with additional data used by specific combinator implementations.
pub struct StateAndData<'a, T, Data: ?Sized> {
    pub state: State<'a, T>,
    pub data: Data,
}

/// A trait implemented by every type.
pub trait Any {}
impl<T: ?Sized> Any for T {}

/// An eager future representing an asynchronous value.
pub struct Eager<'a, T>(pub Rc<RefCell<StateAndData<'a, T, dyn Any + 'a>>>);

impl<'a, T> Clone for Eager<'a, T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<'a, T> Eager<'a, T> {
    /// Create a new eager future. Intended to be used by I/O operation implementers.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(StateAndData {
            state: State::New,
            data: (),
        })))
    }

    /// Setup the eager future so that when it completes, discard the result and do nothing.
    pub fn set_no_handler(&self) {
        let mut s = self.0.borrow_mut();
        if let State::New = s.state {
            s.state = State::NoHandler
        } else {
            panic!("handler has already been setup")
        }
    }

    /// Setup the eager future so that when it completes, call the supplied handler.
    pub fn set_handler(&self, handler: impl FnOnce(T) + 'a) {
        let mut s = self.0.borrow_mut();
        if let State::New = s.state {
            s.state = State::HasHandler(Box::new(handler))
        } else {
            panic!("handler has already been setup")
        }
    }

    /// Complete the eager future. Intended to be used by I/O operation implementers.
    pub fn on_complete(&self, result: T) {
        match std::mem::replace(&mut self.0.borrow_mut().state, State::Completed) {
            State::New => panic!("handler hasn't been setup yet"),
            State::NoHandler => (),
            State::HasHandler(handler) => handler(result),
            State::Completed => panic!("already completed"),
        }
    }

    /// Transform the result of this eager future by the computation defined by `func`.
    pub fn map<To, F>(&self, func: F) -> Eager<'a, To>
    where
        To: 'a,
        F: FnOnce(T) -> To + 'a,
    {
        let result = Eager::new();
        let clone = result.clone();
        self.set_handler(move |x| clone.on_complete(func(x)));
        result
    }

    /// Chain another operation defined by `func` after this eager future completes.
    pub fn then<To, F>(&self, func: F) -> Eager<'a, To>
    where
        To: 'a,
        F: FnOnce(T) -> Eager<'a, To> + 'a,
    {
        let result = Eager::new();
        let clone = result.clone();
        self.set_handler(move |x| func(x).set_handler(move |x| clone.on_complete(x)));
        result
    }
}

struct AllData<T> {
    results: Vec<Option<T>>,
    n_remain: usize,
}

/// Return an eager future that completes after all of the input eager futures complete.
/// The output of the returned eager future is the collected output of the input eager futures.
pub fn all<'a: 'b, 'b, T: 'a>(
    iter: impl IntoIterator<Item = &'b Eager<'a, T>>,
) -> Eager<'a, Vec<T>> {
    let result = Rc::new(RefCell::new(StateAndData {
        state: State::New,
        data: AllData {
            results: Vec::new(),
            n_remain: 0,
        },
    }));
    let mut i = 0;
    for x in iter {
        let clone = result.clone();
        x.set_handler(move |x| {
            let mut s = clone.borrow_mut();
            s.data.results[i] = Some(x);
            s.data.n_remain -= 1;
            if s.data.n_remain == 0 {
                let results = std::mem::take(&mut s.data.results)
                    .into_iter()
                    .map(|x| x.unwrap())
                    .collect();
                std::mem::drop(s);
                Eager(clone).on_complete(results);
            }
        });
        i += 1;
    }
    assert_ne!(i, 0);
    let mut s = result.borrow_mut();
    s.data.results.resize_with(i, || None);
    s.data.n_remain = i;
    std::mem::drop(s);
    Eager(result)
}

/// Return an eager future that completes after any of the input eager futures complete.
/// The output of the returned eager future is the output of the first eager futures completed.
pub fn any<'a: 'b, 'b, T: 'a>(iter: impl IntoIterator<Item = &'b Eager<'a, T>>) -> Eager<'a, T> {
    let result = Rc::new(RefCell::new(StateAndData {
        state: State::New,
        data: true,
    }));
    for x in iter {
        let clone = result.clone();
        x.set_handler(move |x| {
            let mut s = clone.borrow_mut();
            if s.data {
                s.data = false;
                std::mem::drop(s);
                Eager(clone).on_complete(x);
            }
        });
    }
    Eager(result)
}

#[cfg(test)]
mod tests {
    use super::{all, any, Eager};

    #[test]
    fn map_works() {
        let mut handled = false;
        let source = Eager::<i32>::new();
        source.map(|x| x + 1).set_handler(|x| {
            assert_eq!(x, 3);
            handled = true;
        });
        source.on_complete(2);
        std::mem::drop(source);
        assert!(handled);
    }

    #[test]
    fn then_works() {
        let mut handled = false;
        let source = Eager::<i32>::new();
        let next = Eager::<bool>::new();
        let clone = next.clone();
        source
            .then(|x| {
                assert_eq!(x, 5);
                clone
            })
            .set_handler(|x| {
                assert!(x);
                handled = true;
            });
        source.on_complete(5);
        next.on_complete(true);
        std::mem::drop(source);
        std::mem::drop(next);
        assert!(handled);
    }

    #[test]
    fn all_works() {
        let mut output = None;
        let mut sources = Vec::new();
        sources.resize_with(3, || Eager::new());
        all(&sources)
            .map(|x| x.into_iter().sum())
            .set_handler(|x| output = Some(x));
        sources[0].on_complete(1);
        sources[1].on_complete(4);
        sources[2].on_complete(5);
        std::mem::drop(sources);
        assert_eq!(output, Some(10));
    }

    #[test]
    fn any_works() {
        let mut output = None;
        let mut sources = Vec::new();
        sources.resize_with(3, || Eager::new());
        any(&sources).set_handler(|x| output = Some(x));
        sources[0].on_complete(2);
        sources[1].on_complete(5);
        std::mem::drop(sources);
        assert_eq!(output, Some(2));
    }
}
