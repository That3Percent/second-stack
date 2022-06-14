mod stack;
use stack::Stack;
use std::cell::UnsafeCell;

use std::{self, mem::MaybeUninit};
use std::{mem, slice};

thread_local!(
    static THREAD_LOCAL_POOL: UnsafeCell<Stack> = UnsafeCell::new(
        Stack::null()
    )
);

/// WARNING: This is currently only implemented for types with a layout equal to T: u8.
/// Any other type will panic.
pub fn uninit_slice<T, F, R>(len: usize, f: F) -> R
where
    F: FnOnce(&mut [MaybeUninit<T>]) -> R,
{
    // Layout currently not implemented for anything that is not like u8
    assert!(mem::size_of::<T>() == 1 && mem::align_of::<T>() == 1);

    // Special case for ZST that disregards the rest of the code,
    // so that none of that code need account for ZSTs.
    if std::mem::size_of::<T>() == 0 {
        let mut zst_buf = Vec::with_capacity(len);
        for _ in 0..len {
            zst_buf.push(MaybeUninit::uninit());
        }
        return f(&mut zst_buf);
    }

    // Optimization for small slices. This is currently required for correctness
    // See also 26936c11-5b7c-472e-8f63-7922e63a5425
    // TODO: It would be nice to also check that T is small, but since this
    // is required for correctness we cannot presently do that.
    if len <= 32 {
        let mut slice = [
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
        ];
        return f(&mut slice[..len]);
    }

    // Get a ptr representing the new slice, and the old allocation to
    // restore once the function is finished running.
    let (restore, ptr) = THREAD_LOCAL_POOL.with(|cell| unsafe {
        let mut stack = &mut *cell.get();
        let remaining_bytes = stack.capacity - stack.len;
        if remaining_bytes < len {
            let mut capacity = 64.max(stack.capacity * 2);
            while capacity < len {
                capacity *= 2;
            }
            let mut dealloc = mem::replace(stack, Stack::new(capacity));
            // This line is needed just in case the previous
            // stack was not currently borrowed.
            dealloc.try_dealloc();
        }

        let restore = stack.clone();

        let ptr = stack.base.offset(stack.len as isize);
        stack.len += len;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn default_checks<F>(len: usize, f: F)
    where
        F: FnOnce(),
    {
        uninit_u8_slice(len, |uninit| {
            assert_eq!(len, uninit.len());
            for i in 0..uninit.len() {
                uninit[i] = MaybeUninit::new((i % 256) as u8);
            }
            f();
            let init = unsafe { &*(uninit as *const [MaybeUninit<u8>] as *const [u8]) };
            // Verify that nothing overwrote this array.
            for i in 0..init.len() {
                assert_eq!(init[i], (i % 256) as u8);
            }
        })
    }

    fn uninit_u8_slice<F>(len: usize, f: F)
    where
        F: FnOnce(&mut [MaybeUninit<u8>]),
    {
        uninit_slice::<u8, _, _>(len, f)
    }

    #[test]
    fn alloc_is_correct_len() {
        for _ in 0..2 {
            for len in [0, 2, 10, 15, 32, 33, 60, 65, 100, 200] {
                default_checks(len, || ());
            }
        }
    }

    #[test]
    fn recursive_alloc() {
        for _ in 0..2 {
            default_checks(256, || {
                default_checks(1024, || {
                    default_checks(15, || {
                        default_checks(64, || default_checks(1024, || default_checks(999, || ())))
                    })
                })
            });
        }
    }
}
