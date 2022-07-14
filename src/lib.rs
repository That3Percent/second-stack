mod allocation;
use allocation::Allocation;

use std::{
    self,
    cell::UnsafeCell,
    mem::{size_of, MaybeUninit},
    ptr, slice,
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
        unsafe {
            stack.force_dealloc();
        }
    }
}

impl Stack {
    pub fn new() -> Self {
        Self(UnsafeCell::new(Allocation::null()))
    }

    /// Place a potentially very large value on this stack.
    pub fn uninit<T, R, F>(&self, f: F) -> R
    where
        F: FnOnce(&mut MaybeUninit<T>) -> R,
    {
        // Delegate implementation to uninit_slice just to get this working.
        // Performance could be slightly improved with a bespoke implementation
        // of this method.
        self.uninit_slice(1, |slice| f(&mut slice[0]))
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
            let mut tmp = Vec::<T>::with_capacity(len);
            // We do need to take a slice here, because suprisingly
            // tmp.capacity() returns 18446744073709551615
            let slice = &mut tmp.spare_capacity_mut()[..len];
            return f(slice);
        }

        // Required for correctness
        // See also: 26936c11-5b7c-472e-8f63-7922e63a5425
        if len == 0 {
            return f(&mut []);
        }

        // Get the new slice, and the old allocation to
        // restore once the function is finished running.
        let (_restore, (ptr, len)) = unsafe {
            let stack = &mut *self.0.get();
            stack.get_slice(&self.0, len)
        };

        let slice = unsafe { slice::from_raw_parts_mut(ptr as *mut MaybeUninit<T>, len) };

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
            base: *mut T,
            len: usize,
            capacity: usize,
        }

        impl<T> Writer<'_, T> {
            unsafe fn write(&mut self, item: T) {
                self.base.add(self.len).write(item);
                self.len += 1;
            }

            fn try_reuse(&mut self, stack: &mut Allocation) -> bool {
                if let Some(prev) = &self.restore {
                    if prev.restore.ref_eq(stack) {
                        // If we are already are using this stack, we know the
                        // end ptr is already aligned. To double in size,
                        // we would need as many bytes as there are currently
                        // and do not need to align
                        let required_bytes = size_of::<T>() * self.capacity;

                        if stack.remaining_bytes() >= required_bytes {
                            stack.len += required_bytes;
                            self.capacity *= 2;
                            return true;
                        }
                    }
                }
                false
            }
        }

        impl<T> Drop for Writer<'_, T> {
            fn drop(&mut self) {
                unsafe {
                    for i in 0..self.len {
                        self.base.add(i).drop_in_place()
                    }
                }
            }
        }

        unsafe {
            let mut writer = Writer {
                restore: None,
                base: ptr::null_mut(),
                capacity: 0,
                len: 0,
            };

            for next in i {
                if writer.capacity == writer.len {
                    let stack = &mut *self.0.get();

                    // First try to use the same stack, but if that fails
                    // copy over to the upsized stack
                    if !writer.try_reuse(stack) {
                        // This will always be a different allocation, otherwise
                        // try_reuse would have succeeded
                        let (restore, (base, capacity)) =
                            stack.get_slice(&self.0, (writer.len * 2).max(1));

                        // Check for 0 is to avoid copy from null ptr (miri violation)
                        if writer.len != 0 {
                            ptr::copy_nonoverlapping(writer.base, base, writer.len);
                        }

                        // This attempts to restore the old allocation when
                        // writer.restore is Some, but we know that there
                        // is a new allocation at this point, so the only
                        // thing it can do is free memory
                        writer.restore = Some(restore);

                        writer.capacity = capacity;
                        writer.base = base;
                    }
                }
                writer.write(next);
            }

            // TODO: (Performance?) Drop reserve of unused stack, if any. We have over-allocated.
            // TODO: (Performance?) Consider using size_hint

            let buffer = slice::from_raw_parts_mut(writer.base, writer.len);
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

/// Place a potentially very large value on the threadlocal second stack.
pub fn uninit<T, F, R>(f: F) -> R
where
    F: FnOnce(&mut MaybeUninit<T>) -> R,
{
    THREAD_LOCAL.with(|stack| stack.uninit(f))
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
