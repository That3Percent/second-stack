mod allocation;
use allocation::Allocation;

use std::{
    self,
    cell::UnsafeCell,
    mem::{size_of, MaybeUninit},
    slice,
};

thread_local!(
    static THREAD_LOCAL: Stack = Stack::new()
);

/// A Stack that is managed separately from the threadlocal one.
/// Typically, using the threadlocal APIs
/// is encouraged because they enable sharing across libraries, where each
/// re-use lowers the amortized cost of maintaining allocations. But, if
/// full control is necessary this API may be used.
pub struct Stack(UnsafeCell<Allocation>);

impl Drop for Stack {
    fn drop(&mut self) {
        let stack = self.0.get_mut();
        // It's ok to use force_dealloc here instead of try_dealloc
        // because we know the allocation cannot be in-use. By eliding
        // the check, this allows the allocation to be freed when there
        // was a panic
        stack.force_dealloc();
    }
}

// Copied from the nightly feature MaybeUninit::assume_init
const fn uninit_array<const N: usize, T>() -> [MaybeUninit<T>; N] {
    // SAFETY: An uninitialized `[MaybeUninit<_>; LEN]` is valid.
    unsafe { MaybeUninit::<[MaybeUninit<T>; N]>::uninit().assume_init() }
}

impl Stack {
    pub fn new() -> Self {
        Self(UnsafeCell::new(Allocation::null()))
    }

    /// Allocates an uninit slice from this stack.
    pub fn uninit_slice<T, F, R>(&self, len: usize, f: F) -> R
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

        // Get the new slice, and the old allocation to
        // restore once the function is finished running.
        let (_restore, slice) = unsafe {
            let stack = &mut *self.0.get();
            stack.get_slice(&self.0, len)
        };

        f(slice)
    }

    /// Buffers an iterator to a slice on this stack and gives temporary access to that slice.
    /// Do not use with an unbounded iterator, because this will eventually run out of memory and panic.
    pub fn buffer<T, F, R, I>(&self, i: I, f: F) -> R
    where
        I: Iterator<Item = T>,
        F: FnOnce(&mut [T]) -> R,
    {
        // Special case for ZST
        if size_of::<T>() == 0 {
            let mut v: Vec<_> = i.collect();
            return f(&mut v);
        }

        // Data goes in a struct in case user code panics.
        // User code includes Iterator::next, FnOnce, and Drop::drop
        struct Writer<'a, T> {
            restore: Option<DropStack<'a>>,
            reserved: &'a mut [MaybeUninit<T>],
            written: usize,
        }

        impl<T> Writer<'_, T> {
            fn write(&mut self, item: T) {
                self.reserved[self.written] = MaybeUninit::new(item);
                self.written += 1;
            }
            fn inits(&mut self) -> &mut [MaybeUninit<T>] {
                &mut self.reserved[..self.written]
            }

            fn try_reuse(&mut self, stack: &mut Allocation) -> bool {
                unsafe {
                    if let Some(prev) = &self.restore {
                        if prev.restore.ref_eq(stack) {
                            // If we are already are using this stack, we know the
                            // end ptr is already aligned. To double in size,
                            // we would need as many bytes as there are currently
                            // and do not need to align
                            let required_bytes = size_of::<T>() * self.reserved.len();

                            if stack.remaining_bytes() >= required_bytes {
                                stack.len += required_bytes;

                                self.reserved = slice::from_raw_parts_mut(
                                    self.reserved.as_mut_ptr() as *mut MaybeUninit<T>,
                                    self.written * 2,
                                );
                                return true;
                            }
                        }
                    }
                }
                false
            }
        }

        impl<T> Drop for Writer<'_, T> {
            fn drop(&mut self) {
                unsafe {
                    for init in self.inits() {
                        init.assume_init_drop();
                    }
                }
            }
        }

        unsafe {
            let mut on_stack: [MaybeUninit<T>; 32] = uninit_array();
            let mut writer = Writer {
                restore: None,
                reserved: &mut on_stack,
                written: 0,
            };

            for next in i {
                if writer.written == writer.reserved.len() {
                    let stack = &mut *self.0.get();

                    // First try to use the same stack, but if that fails
                    // copy over to the upsized stack
                    if !writer.try_reuse(stack) {
                        // This will always be a different allocation, otherwise
                        // try_reuse would have succeeded
                        let (restore, slice) = stack.get_slice(&self.0, writer.written * 2);

                        for i in 0..writer.written {
                            slice[i].write(writer.reserved[i].assume_init_read());
                        }
                        // This attempts to restore the old allocation when
                        // writer.restore is Some, but we know that there
                        // is a new allocation at this point, so the only
                        // thing it can do is free memory
                        writer.restore = Some(restore);
                        writer.reserved = slice;
                    }
                }
                writer.write(next);
            }

            // TODO: (Performance?) Drop reserve of unused stack, if any. We have over-allocated.
            // TODO: (Performance?) Consider using size_hint

            let inits = writer.inits();

            // This is copied from slice_assume_init_mut, which is
            // currently an unstable API
            let buffer = &mut *(inits as *mut [MaybeUninit<T>] as *mut [T]);

            f(buffer)
        }
    }
}

/// Allocates an uninit slice from the threadlocal stack.
pub fn uninit_slice<T, F, R>(len: usize, f: F) -> R
where
    F: FnOnce(&mut [MaybeUninit<T>]) -> R,
{
    THREAD_LOCAL.with(|stack| stack.uninit_slice(len, f))
}

/// Buffers an iterator to a slice on the threadlocal stack and gives temporary access to that slice.
/// Panics when running out of memory if the iterator is unbounded.
pub fn buffer<T, F, R, I>(i: I, f: F) -> R
where
    I: Iterator<Item = T>,
    F: FnOnce(&mut [T]) -> R,
{
    THREAD_LOCAL.with(|stack| stack.buffer(i, f))
}

// The logic to drop our Allocation goes into a drop impl so that if there
// is a panic the drop logic is still run and we don't leak any memory.
pub(crate) struct DropStack<'a> {
    pub restore: Allocation,
    pub location: &'a UnsafeCell<Allocation>,
}

impl Drop for DropStack<'_> {
    fn drop(&mut self) {
        unsafe {
            let mut current = &mut *self.location.get();
            if current.ref_eq(&self.restore) {
                current.len = self.restore.len;
            } else {
                self.restore.try_dealloc();
            }
        }
    }
}
