mod stack;
use stack::Stack;
use std::cell::UnsafeCell;

use std::ops::{Deref, DerefMut};
use std::{self, mem::MaybeUninit};
use std::{
    mem::{align_of, replace, size_of},
    slice,
};

thread_local!(
    static THREAD_LOCAL_POOL: Dropper = Dropper(UnsafeCell::new(
        Stack::null()
    ))
);

struct Dropper(UnsafeCell<Stack>);
impl Drop for Dropper {
    fn drop(&mut self) {
        let stack = self.get_mut();
        stack.force_dealloc();
    }
}
impl Deref for Dropper {
    type Target = UnsafeCell<Stack>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for Dropper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// Copied from the nightly feature MaybeUninit::assume_init
const fn uninit_array<const N: usize, T>() -> [MaybeUninit<T>; N] {
    // SAFETY: An uninitialized `[MaybeUninit<_>; LEN]` is valid.
    unsafe { MaybeUninit::<[MaybeUninit<T>; N]>::uninit().assume_init() }
}

/// Allocates an uninit slice from the threadlocal stack, resizing if necessary.
pub fn uninit_slice<T, F, R>(len: usize, f: F) -> R
where
    F: FnOnce(&mut [MaybeUninit<T>]) -> R,
{
    // Special case for ZST that disregards the rest of the code,
    // so that none of that code need account for ZSTs.
    // The reason this is convenient is that a ZST may use
    // the stack without bumping the pointer, which will
    // lead other code to free that memory while still in-use.
    // See also: 2ec61cda-e074-4b26-a9a5-a01b70706585
    // There may be other issues also.
    if std::mem::size_of::<T>() == 0 {
        let mut tmp = Vec::<MaybeUninit<T>>::with_capacity(len);
        let tmp = tmp.as_mut_ptr();
        let mut slice = unsafe { slice::from_raw_parts_mut(tmp, len) };
        return f(&mut slice);
    }

    // Optimization for small slices. This is currently required for correctness
    // See also: 26936c11-5b7c-472e-8f63-7922e63a5425
    // TODO: It would be nice to also check that T is small, but since this
    // is required for correctness we cannot presently do that.
    if len <= 32 {
        let mut slice: [MaybeUninit<T>; 32] = uninit_array();
        return f(&mut slice[..len]);
    }

    // Get a ptr representing the new slice, and the old allocation to
    // restore once the function is finished running.
    let (restore, ptr) = THREAD_LOCAL_POOL.with(|cell| unsafe {
        let mut stack = &mut *cell.get();
        let remaining_bytes = stack.capacity - stack.len;
        // Requires at a minimum size * len, but at a maximum must also pay
        // an alignment cost.
        let required_bytes_pessimistic = (align_of::<T>() - 1) + (size_of::<T>() * len);
        if remaining_bytes < required_bytes_pessimistic {
            // Require at least 64 bytes for the smallest allocation,
            // and require we at least double in size from the previous
            // allocated stack
            let mut capacity = 64.max(stack.capacity * 2);
            // Require that we are a power of 2 and can fit
            // the desired slice.
            while capacity < required_bytes_pessimistic {
                capacity *= 2;
            }
            let mut dealloc = replace(stack, Stack::new(capacity));
            // If the previous stack was not borrowed, we need to
            // free it.
            dealloc.try_dealloc();
        }

        let restore = stack.clone();

        let base = stack.base.offset(stack.len as isize);
        let align = base.align_offset(align_of::<T>());
        let ptr = base.offset(align as isize);
        stack.len += align + (size_of::<T>() * len);

        (restore, ptr)
    });
    let _restore = DropStack(restore);

    let slice = unsafe { slice::from_raw_parts_mut(ptr as *mut MaybeUninit<T>, len) };
    let result = f(slice);
    drop(slice);

    // The logic to drop our Stack goes into a drop impl so that if f() panics,
    // the drop logic is still run and we don't leak any memory.
    struct DropStack(Stack);
    impl Drop for DropStack {
        fn drop(&mut self) {
            unsafe {
                // TODO: Would be nice to use the nightly feature with_borrow_mut
                THREAD_LOCAL_POOL.with(|cell| {
                    let mut current = &mut *cell.get();
                    if current.ref_eq(&self.0) {
                        current.len = self.0.len;
                    } else {
                        self.0.try_dealloc();
                    }
                });
            }
        }
    }

    result
}

/*
pub fn iterator<T, F, R, I>(i: I, f: F) -> R
where
    I: Iterator<Item = T>,
    F: FnOnce(&[T]) -> R,
{
    todo!()
}
*/
