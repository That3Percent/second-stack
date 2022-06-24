mod allocation;
use allocation::Allocation;

use std::{
    self,
    cell::UnsafeCell,
    mem::{size_of, MaybeUninit},
    ops::{Deref, DerefMut},
    slice,
};

thread_local!(
    static THREAD_LOCAL: Dropper = Dropper(UnsafeCell::new(
        Allocation::null()
    ))
);

struct Dropper(UnsafeCell<Allocation>);
impl Drop for Dropper {
    fn drop(&mut self) {
        let stack = self.get_mut();
        stack.force_dealloc();
    }
}

impl Deref for Dropper {
    type Target = UnsafeCell<Allocation>;
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

    // Get the new slice, and the old allocation to
    // restore once the function is finished running.
    let (_restore, slice) = THREAD_LOCAL.with(|cell| unsafe {
        let stack = &mut *cell.get();
        stack.get_slice(len)
    });

    f(slice)
}

/// Buffers an iterator to a slice and gives temporary access to that slice.
/// Panics when running out of memory if the iterator is unbounded.
pub fn buffer<T, F, R, I>(i: I, f: F) -> R
where
    I: Iterator<Item = T>,
    F: FnOnce(&mut [T]) -> R,
{
    // Special case for ZST
    if size_of::<T>() == 0 {
        let mut v = Vec::new();
        for item in i {
            v.push(item);
        }
        return f(&mut v);
    }

    // Data goes in a struct in case user code panics.
    // User code includes Iterator::next, FnOnce, and Drop::drop
    struct Writer<'a, T> {
        restore: Option<DropStack>,
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
                THREAD_LOCAL.with(|cell| {
                    let stack = &mut *cell.get();

                    // First try to use the same stack
                    if let Some(prev) = &writer.restore {
                        if prev.0.base == stack.base {
                            // If we are already are using this stack, we know the
                            // end ptr is already aligned. To double in size,
                            // we would need as many bytes as there are currently
                            // and do not need to align
                            let required_bytes = size_of::<T>() * writer.reserved.len();

                            if stack.remaining_bytes() >= required_bytes {
                                stack.len += required_bytes;

                                writer.reserved = slice::from_raw_parts_mut(
                                    writer.reserved.as_mut_ptr() as *mut MaybeUninit<T>,
                                    writer.written * 2,
                                );
                                return;
                            }
                        }
                    }

                    let (restore, slice) = stack.get_slice(writer.written * 2);

                    for i in 0..writer.written {
                        slice[i].write(writer.reserved[i].assume_init_read());
                    }
                    // This attempts to restore the old allocation when
                    // writer.restore is Some, but we know that there
                    // is a new allocation at this point, so it may just
                    // free memory.
                    writer.restore = Some(restore);
                    writer.reserved = slice;
                });
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

// The logic to drop our Allocation goes into a drop impl so that if there
// is a panic the drop logic is still run and we don't leak any memory.
pub(crate) struct DropStack(Allocation);
impl Drop for DropStack {
    fn drop(&mut self) {
        unsafe {
            THREAD_LOCAL.with(|cell| {
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
