# eager-futures

### Introduction
This crate implements completion-handler based futures.
It's intended for single-threaded asynchronous programming with reactor / proactor pattern,
similar to the asynchrony model of *Node.js*.
### Motivation
In Rust's native [`future`](std::future), when a sub-future completes,
the root task is awakened, which recursively polls children futures until reaching the true point of reentrance.
In the future provided by this crate, when a sub-future completes,
the next future to run is directly awakened, rather than having to be polled by parent futures.
Essentially, each future is its own root task. This resembles Javascript's Promises,
where they immediately start execution upon creation. Hence the future type provided by this create is named [`Eager`].

Consider the following code using Rust's native future. It simply chains `n` futures together using `then`,
where each future performs some I/O operation simulated here by `yield_now`.
One would expect the time for running it to be linear to `n`, but actually it's quadratic to `n`
because each time the I/O operation completes, we have to poll through the `next` chain from the beginning.
The future provided by this crate doesn't exhibit this behavior.
```rust
fn chain_many(n: isize) -> BoxFuture<'static, isize> {
    let mut future = async { 0 }.boxed();
    for _ in 0..n {
        future = future.then(|x| async move {
            tokio::task::yield_now().await;
            x + 1
        }).boxed()
    }
    future
}
```
### Notes
Compared to Rust's native futures, this approach introduces dynamic allocation overhead.
This crate is suitable only if your control flow is dynamically composed such as in the example above.
For control flow that is mostly static, Rust's native future could perform much better
thanks to optimizations such as aggressive inlining. This crate doesn't work with `async` / `await`.

License: BSD-2-Clause
