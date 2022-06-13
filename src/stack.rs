#[cfg(test)]
use std::thread;
use std::{mem, ptr};

#[derive(Clone)]
pub struct Stack {
    pub base: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl Stack {
    pub fn ref_eq(&self, other: &Self) -> bool {
        self.base == other.base
    }
    pub fn null() -> Self {
        Self {
            base: ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }

    pub fn new(size_in_bytes: usize) -> Self {
        let mut v = Vec::<u8>::with_capacity(size_in_bytes);
        let base = v.as_mut_ptr();
        mem::forget(v);

        #[cfg(test)]
        println!(
            "second-stack allocated {size_in_bytes} bytes at {base:?} on thread {:?}",
            thread::current().id()
        );

        Self {
            base,
            len: 0,
            capacity: size_in_bytes,
        }
    }

    pub fn try_dealloc(&mut self) {
        // Don't dealloc if the slice is in-use.
        // We assume at this point that there are no slices with len
        // 0 in-use, because we don't use the Stack type for those.
        // See also 26936c11-5b7c-472e-8f63-7922e63a5425
        if self.len != 0 {
            return;
        }
        // Right now this should be unnecessary protection. But, this
        // code is infrequently called.
        if self.base == ptr::null_mut() {
            return;
        }

        unsafe {
            #[cfg(test)]
            println!(
                "second-stack deallocated {} bytes at {:?} on thread {:?}",
                self.capacity,
                self.base,
                thread::current().id()
            );
            // Drops the memory
            drop(Vec::from_raw_parts(self.base, 0, self.capacity));
        }

        self.base = ptr::null_mut();
    }
}