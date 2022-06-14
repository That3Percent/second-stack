`second-stack` is an allocator for large objects which do not need to be long-lived.

The internal representation is a thread local stack that grows when necessary. Once the capacity saturates, the same allocation will be re-used for many consumers, making it more efficient each time it is used.

The implementation is currently in the middle of a re-write away from using Rust nightly APIs, and does not yet have the full set of capabilities from the previous version. In particular, there is a need for initializing a slice from an iterator, and support for more layouts than the same as u8 (anything else will currently panic)