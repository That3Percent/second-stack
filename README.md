`second-stack` is an allocator for short-lived slices which may not fit on the thread's stack. It is often faster to use second-stack over `Vec` for the same reason using the thread's stack is faster than using the heap most of the time.

The internal representation is a thread local stack that grows as necessary. Once the capacity saturates, the same allocation will be re-used for many consumers, making it more efficient the more libraries adopt it.

`second-stack` was originally developed for writing dynamic buffers in WebGL (eg: procedurally generate some triangles/colors, write them to a buffer, and hand them off to the graphics card many times per frame without incurring the cost of many heap allocations). But, over time I found that needing a short-lived slice was common and using `second-stack` all over the place allowed for the best memory re-use and performance.

> We've had one, yes. What about second stack?

There are two ways to use this API. The preferred way is to use methods which delegate to a shared thread local. Using these methods ensures that multiple libraries efficiently re-use allocations without passing around context and exposing this implementation detail in their public API. Alternatively, you can use `Stack::new()` to create your own managed stack if you need more control.

Example using buffer:
```rust
// Buffer takes any iterator, writes it to a slice on the second stack, and gives you mutable access to the slice.
buffer(0..1000, |items| {
    assert_eq!(items.len(), 1000);
    assert_eq!(items[19], 19);
})
```

Example using uninit_slice:
```rust
uninit_slice(|slice| {
    // Do some unsafe stuff here
})
```