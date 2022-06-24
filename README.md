`second-stack` is an allocator for large objects which do not need to be long-lived.

The internal representation is a thread local stack that grows when necessary. Once the capacity saturates, the same allocation will be re-used for many consumers, making it more efficient each time it is used.