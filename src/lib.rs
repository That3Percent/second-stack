#![feature(allocator_api, alloc_layout_extra, ptr_offset_from)]

use std::alloc::*;
use std::cell::RefCell;
use std::iter::*;
use std::mem::{align_of, size_of};
use std::ops::{Deref, DerefMut};
use std::{ptr, slice};

/// A chunk of memory that the StackPool will allocate from, as well as the pointer
/// that indicates how much (if any) is used. The default value for this struct is
/// unallocated, containing only null pointers.
struct Heap {
    bottom: *mut u8, // invariant
    top: *mut u8,
    current: *mut u8,
}


/// At some point, it was decided that offset_from should always return the invalid value in MIRI. Sidestep that.
/// https://github.com/rust-lang/rust/issues/62420
fn guaranteed_align_offset<T>(from: *mut u8) -> usize {
	let n = from as usize;
	((n + align_of::<T>() - 1) & !(align_of::<T>() - 1)) - n
}

/// Bumps a pointer up to the nearest aligned address
fn align<T>(value: *mut u8) -> *mut u8 {
	let n = value as usize;
	((n + align_of::<T>() - 1) & !(align_of::<T>() - 1)) as *mut u8
}

impl Heap {
    /// How many of T can we hold right now?
    /// This takes into account not just the size of T, but also the alignment of our current stack pointer.
    pub fn usable_size<T>(&self) -> usize {
		match size_of::<T>() {
			0 => std::isize::MAX as usize,
			_ => {
				let lost_to_alignment = guaranteed_align_offset::<T>(self.current) as isize;
        		let bytes_remaining = unsafe { self.top.offset_from(self.current) } - lost_to_alignment;
				if bytes_remaining <= 0 {
					0
				} else {
					bytes_remaining as usize / size_of::<T>()
				}
			}
		}
    }

    /// Returns a smart pointer to a slice from this allocation, bumping up our stack pointer in the process.
    pub fn slice<T>(&mut self, count: usize, pool_index: usize) -> StackAlloc<T> {
        debug_assert!(self.usable_size::<T>() >= count);
        let layout = Layout::new::<T>().repeat(count).unwrap().0;
        let restore = self.current;
        let base = align::<T>(restore);
        self.current = unsafe { base.add(layout.size()) };
        StackAlloc {
            len: count,
            ptr: base as *mut T,
            restore,
            pool_alloc: pool_index,
        }
    }

    /// Convenience function for the layout of a [u8]
    fn layout_u8(size_in_bytes: usize) -> Layout {
        Layout::new::<u8>().repeat(size_in_bytes).unwrap().0 // TODO: Use layout more elsewhere, it was discovered late.
    }

    /// Does an allocation belong to this pool?
    #[cfg(debug_assertions)]
    fn contains(&self, p: *mut u8) -> bool {
        unsafe {
            !self.bottom.is_null()
                && p.offset_from(self.bottom) >= 0
                && self.top.offset_from(p) >= 0
        }
    }

    /// Free an allocation from StackAlloc
    pub fn release(&mut self, p: *mut u8) {
        #[cfg(debug_assertions)]
        debug_assert!(self.contains(p));
        // This is redundant with the full history check, but then again all debug asserts should be redundant.
        unsafe {
            debug_assert!(p.offset_from(self.current) <= 0, "Out of order release");
        }
        self.current = p;
    }

    /// The size of all slices in use. This is <= bytes_total()
    pub fn bytes_used(&self) -> usize {
        unsafe { self.current.offset_from(self.bottom) as usize }
    }

    /// The total size of this allocation, free and used.
    pub fn bytes_total(&self) -> usize {
        unsafe { self.top.offset_from(self.bottom) as usize }
    }

    /// Creates a new Heap of a specific size.
    pub fn new(size_in_bytes: usize) -> Heap {
        // TODO: Use error_chain instead of unwrap. https://docs.rs/error-chain/0.12.0/error_chain/
        debug_assert!(size_in_bytes >= size_for_i(0));
        let layout = Self::layout_u8(size_in_bytes);
        debug_assert!(size_in_bytes == layout.size()); // Invariant, See also: c4e1285a-306a-450f-a027-13c0cd3d3d08
        unsafe {
            let bottom = Global.alloc(layout).unwrap().as_ptr(); // TODO: There may be mitigation, like not doubling in size.
            Heap {
                bottom,
                current: bottom,
                top: bottom.add(layout.size()),
            }
        }
    }
}

impl Drop for Heap {
    /// Free memory when the pool is unowned.
    fn drop(&mut self) {
        if let Some(bottom) = ptr::NonNull::new(self.bottom) {
        	debug_assert!(self.bytes_used() == 0, "bytes used {} > 0", self.bytes_used()); // Do not free memory if still in-use. Not runtime check, because this should be statically impossible if this module is implemented correctly.
            // May be null if the Heap was unused/unallocated.
            let layout = Self::layout_u8(self.bytes_total()); // See also: c4e1285a-306a-450f-a027-13c0cd3d3d08
            unsafe {
                Global.dealloc(bottom, layout);
            }
        }
    }
}

impl Default for Heap {
    /// The unallocated Heap
    fn default() -> Heap {
        Heap {
            bottom: ptr::null_mut(),
            top: ptr::null_mut(),
            current: ptr::null_mut(),
        }
    }
}

// First pool is 64K in size. We expect to blow right through this, but since this is
// per-thread it can be prudent to start small.
const MIN_POW: usize = 16;
const MAX_POW: usize = 32;
const NUM_POOLS: usize = MAX_POW - MIN_POW; // Since pools at least double in size, there can never be more than this many pools or we would run out of memory for a single allocation.

/// The size of the nth generation of the Heap
fn size_for_i(i: usize) -> usize {
    1 << (i + MIN_POW)
}

/// Holds multiple generations of Heap. Resizes, slices, and frees.
struct StackPool {
    pools: [Heap; NUM_POOLS],
    top: Option<usize>, // Which pool is the top pool?

    #[cfg(debug_assertions)]
    history: Vec<*mut u8>,
}

impl StackPool {
    pub fn get_slice<T>(&mut self, count: usize, i: usize) -> StackAlloc<T> {
        let result = self.pools[i].slice::<T>(count, i);
        #[cfg(debug_assertions)]
        self.history.push(result.restore);
        result
    }
    /// Slice from the top pool, sizing up if necessary.
    pub fn acquire<T>(&mut self, count: usize) -> StackAlloc<T> {
        let pools = &mut self.pools;
        let mut prev_used = 0;
        let mut next_pool = 0;
        if let Some(top) = self.top {
            let pool = &pools[top];
            if pool.usable_size::<T>() > count {
                return self.get_slice(count, top);
            }
            prev_used = pool.bytes_used();
            next_pool = top + 1;
        }
        // The choices are to specify an alignment, or to make sure that the allocation
        // is large enough to accommodate alignment padding. Choosing the latter.
        let min_bytes = prev_used + size_of::<T>() * count + align_of::<T>();
        for i in next_pool..NUM_POOLS {
            let size = size_for_i(i);
            if size_for_i(i) >= min_bytes {
                pools[i] = Heap::new(size);
                self.top = Some(i);
                return self.get_slice(count, i);
            }
        }
        panic!("Allocation size too large");
    }

    /// Release a previously acquired pointer.
    pub fn release<T>(&mut self, ptr: &StackAlloc<T>) {
        #[cfg(debug_assertions)]
        debug_assert!(self.history.pop().unwrap() == ptr.restore);

        let pool = &mut self.pools[ptr.pool_alloc];
		pool.release(ptr.restore);
		if (ptr.restore == pool.bottom) && (self.top.unwrap() != ptr.pool_alloc) {
			*pool = Default::default();
        }
    }

    /// How many bytes are allocated by all Heap that are currently owned.
    #[cfg(test)]
    fn total_bytes_allocated(&self) -> usize {
        if let Some(top) = self.top {
            { 0..=top }.map(|i| self.pools[i].bytes_total()).sum()
        } else {
            0
        }
    }
}

// Even though there is no support for threads in wasm now, this is required to make the compiler happy. Even so, threads will be useful later.
thread_local!(
	static THREAD_LOCAL_POOL: RefCell<StackPool> = RefCell::new(
		StackPool {
			pools: Default::default(), top:None,
			#[cfg(debug_assertions)]
			history: Vec::new(),
		}
));

/// A smart pointer that automatically releases the borrowed memory.
#[derive(Debug)]
pub struct StackAlloc<T> {
    restore: *mut u8,
    ptr: *mut T,
    len: usize,
    pool_alloc: usize,
}

impl<T> Deref for StackAlloc<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        unsafe { slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl<T> DerefMut for StackAlloc<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl<T> Drop for StackAlloc<T> {
    /// Release our slice from the Heap when no longer owned.
    fn drop(&mut self) {
        unsafe {
            ptr::drop_in_place(&mut self[..]);
        }
        THREAD_LOCAL_POOL.with(|rc| {
            rc.borrow_mut().release(&self);
        })
    }
}

/// WARNING! The slice that StackAlloc<T> will deref to is logically uninitialized.
/// This leads to all sorts of wildly unsafe things, including undefined behavior.
/// Eg: Acquiring a type that implements drop may cause a write to the slice to drop invalid instances.
// TODO: Add !Drop to signature, but that doesn't seem to be implemented in the compiler yet...
#[cfg(any(test, feature = "experimental"))]
pub unsafe fn acquire_uninitialized<T>(count: usize) -> StackAlloc<T> {
    THREAD_LOCAL_POOL.with(|rc| rc.borrow_mut().acquire(count))
}

/// ## Panics
/// * Must panic if the iterator is unbounded in length, or if the size of the allocation is too large.
pub fn acquire<T, I: Iterator<Item = T>>(items: I) -> StackAlloc<T> {
    THREAD_LOCAL_POOL.with(|rc| {
        let len = items
            .size_hint()
            .1
            .expect("Expected an iterator with an upper bound.");
        // TODO: Check if the size of the allocation would exceed isize.max bytes
        let mut pool = rc.borrow_mut().acquire(len);
        let mut p = pool.ptr;
		debug_assert!(((p as usize) % align_of::<T>()) == 0); // Verify alignment

        // TODO: Decide whether to canonize behavior for a size hint that is too large. (This is probably necessary given that an initializer could allocate.)
        // TODO: Write test for panic in iterator.
        pool.len = 0;
        for item in items.take(len) {
            // Taking only len here ensures that we don't need to trust size-hint.
            unsafe {
                ptr::write(p, item);
                p = p.add(1);
            }
            pool.len += 1; // By modifying the len after writing an item we ensure that uninitialized memory is not dropped.
        }
        // TODO: In the event that an enumerator produces fewer items than the upper bound of the size hint, we should free some
        // memory if possible. Consider though it is possible (if unlikely) that the iterator producing items used allocations
        // from our heap of it's own, so it's not as simple as just moving the pointer down.
        pool
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter::repeat;
    use testdrop::TestDrop;

    #[test]
    fn slices_do_no_alias() {
        let pool0 = acquire(repeat(0).take(10));
        let pool1 = acquire(repeat(1).take(10));

        assert!(pool0.iter().all(|p| *p == 0));
        assert!(pool1.iter().all(|p| *p == 1));
    }

    #[test]
    fn uninitialized_is_correctly_sized() {
        let pool = unsafe { acquire_uninitialized::<u32>(10) };
        assert_eq!(pool.len(), 10);
    }

    #[test]
    fn is_correctly_sized() {
        let pool = acquire(0..10u8);
        assert_eq!(pool.len(), 10);
    }

    #[test]
    fn memory_is_reused() {
        {
            let _ = acquire(0..10usize);
        }
        // The pool is freed here, so we should see the same memory used again.
        {
            let pool1 = unsafe { acquire_uninitialized::<usize>(10) };
            for i in 0..pool1.len() {
                assert_eq!(pool1[i], i);
            }
        }
    }

    // TODO: When there are threads in wasm, add a test to ensure that pools are indeed threadlocal.

    #[test]
    fn memory_is_not_released_eagerly() {
        let current_size = || THREAD_LOCAL_POOL.with(|rc| rc.borrow().total_bytes_allocated());

        // The pre-condition of this test assumes we either have never used the pool, or never upsized it.
        // If this fails we can add a reset method (before wasm threads are introduced), or start up a thread or to get a unique pool, and assert that it's size is 0.
        assert!(current_size() == 0 || current_size() == size_for_i(0));

        let small_size = size_for_i(0) / 2;
        let large_size = size_for_i(0) - 1;

        {
            // After acquiring 1 pool, we should start with the smallest size.
            let _pool0 = unsafe { acquire_uninitialized::<u8>(small_size) };
            assert_eq!(current_size(), size_for_i(0));

            {
                // After requiring something larger, we should have enough space for both pools.
                let _pool1 = unsafe { acquire_uninitialized::<u8>(large_size) };
                assert_eq!(current_size(), size_for_i(0) + size_for_i(1));
            }

            // We released the outer pool, but we should still have the 2nd pool allocated, and we haven't released 0 yet so it should still be there.
            assert_eq!(current_size(), size_for_i(0) + size_for_i(1));
        }

        // We have dropped all slices.
        // The smaller pool should be released, but should still have the largest pool allocated to be ready for the next frame/allocation
        assert_eq!(current_size(), size_for_i(1));
    }

    #[test]
    fn drops() {
        let td = TestDrop::new();
        let (id, item) = td.new_item();
        {
            let some = Some(item);
            let _ = acquire(some.iter());
            // Not dropped when moved into the slice
            td.assert_no_drop(id);
        }

        // Dropped with the slice
        td.assert_drop(id);
    }

    #[test]
    fn shrinks_on_large_size_hint() {
        struct UndersizedIterator {
            remaining: usize,
        }
        impl Iterator for UndersizedIterator {
            type Item = usize;
            fn next(&mut self) -> Option<Self::Item> {
                if self.remaining == 0 {
                    None
                } else {
                    self.remaining -= 1;
                    Some(self.remaining)
                }
            }
            // This is clearly wrong on both the upper and lower bound.
            fn size_hint(&self) -> (usize, Option<usize>) {
                (self.remaining + 20, Some(self.remaining + 20))
            }
        }

        let bad = UndersizedIterator { remaining: 5 };
        let values = acquire(bad);
        assert!(values.len() == 5);
    }

    #[test]
    fn empty_slice_ok() {
        acquire(repeat(0).take(0));
        acquire(repeat(0).take(0));
    }

    #[test]
    fn zst_ok() {
        let data = acquire(repeat(()).take(10));
        debug_assert!(data.len() == 10);
        debug_assert!(data[0] == ());
    }

    /*
    // This doesn't quite work, since it ends up panicking again while unwinding.
    #[test]
    fn release_out_of_order_panics() {
        let result = std::panic::catch_unwind(|| {
            let x = acquire(0..10);
            let y = acquire(0..10);

            fn move_into(ptr: StackAlloc<u8>) {}
            move_into(x);
        });

        assert!(result.is_err());
    }
    */

}
