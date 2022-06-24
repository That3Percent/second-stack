use std::{
    mem::{self, align_of, replace, size_of, MaybeUninit},
    ptr, slice,
};

use crate::DropStack;

#[derive(Clone)]
pub(crate) struct Stack {
    pub base: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl Stack {
    pub fn get_slice<T>(&mut self, len: usize) -> (DropStack, &mut [MaybeUninit<T>]) {
        unsafe {
            // Requires at a minimum size * len, but at a maximum must also pay
            // an alignment cost.
            let required_bytes_pessimistic = (align_of::<T>() - 1) + (size_of::<T>() * len);
            self.ensure_capacity(required_bytes_pessimistic);

            let restore = self.clone();
            let base = self.base.offset(self.len as isize);
            let align = base.align_offset(align_of::<T>());
            let ptr = base.offset(align as isize);
            self.len += align + (size_of::<T>() * len);

            (
                DropStack(restore),
                slice::from_raw_parts_mut(ptr as *mut MaybeUninit<T>, len),
            )
        }
    }
    fn ensure_capacity(&mut self, capacity: usize) {
        if self.remaining_bytes() < capacity {
            // Require at least 64 bytes for the smallest allocation,
            // and require we at least double in size from the previous
            // allocated stack
            let mut new_capacity = 64.max(self.capacity * 2);
            // Require that we are a power of 2 and can fit
            // the desired slice.
            while new_capacity < capacity {
                new_capacity *= 2;
            }
            let mut dealloc = replace(self, Stack::new(new_capacity));
            // If the previous stack was not borrowed, we need to
            // free it.
            dealloc.try_dealloc();
        }
    }

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

    pub fn remaining_bytes(&self) -> usize {
        self.capacity - self.len
    }

    pub fn new(size_in_bytes: usize) -> Self {
        let mut v = Vec::<u8>::with_capacity(size_in_bytes);
        let base = v.as_mut_ptr();
        mem::forget(v);

        // println!("Alloc {size_in_bytes} bytes at {base:?}");

        Self {
            base,
            len: 0,
            capacity: size_in_bytes,
        }
    }

    pub fn force_dealloc(&mut self) {
        if self.base == ptr::null_mut() {
            return;
        }

        unsafe {
            // println!("Dealloc {} bytes at {:?}", self.capacity, self.base,);
            // Deallocates the memory
            drop(Vec::from_raw_parts(self.base, 0, self.capacity));
        }

        self.base = ptr::null_mut();
    }

    pub fn try_dealloc(&mut self) {
        // Don't dealloc if the slice is in-use.
        // We assume at this point that there are no slices with len
        // 0 in-use, because we don't use the Stack type for those.
        // See also: 26936c11-5b7c-472e-8f63-7922e63a5425
        // See also: 2ec61cda-e074-4b26-a9a5-a01b70706585
        if self.len != 0 {
            return;
        }

        self.force_dealloc();
    }
}
