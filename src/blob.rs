use std::{
	alloc::{
		Global,
		Alloc
	},
};

/// Pointers to an allocation and it's current offset.
#[derive(Clone)]
pub struct Blob {
    /// A pointer to where the current offset is stored. This extra layer of indirection
    /// is to allow multiple users of the memory to change the offset.
    ptr: *const *mut u8,
    start: *const u8,
    end: *const u8,
}

impl Blob {
    pub fn is_empty(&self) -> bool {
        self.get_ptr() as *const u8 == self.start
    }

    pub fn get_ptr(&self) -> *mut u8 {
        *self.ptr
    }

    /// Creates a Blob pointing to uninitialized memory with the ptr set to the start.
    pub fn alloc(size_in_bytes: usize) -> Blob {
        unsafe {
            let layout = layout_u8(size_in_bytes);
            let start = Global.alloc(layout).unwrap().as_ptr();
            let ptr = Box::new(start);
            Blob {
                // See also 5082b76d-5db8-4bab-b18d-04e97febc606
                ptr: Box::into_raw(ptr),
                start,
                end: start.add(layout.size()),
            }
        }
	}

	pub fn free() {
		// See also 5082b76d-5db8-4bab-b18d-04e97febc606
		Box::from_raw(*self.ptr);
		let size_in_bytes = self.end.offset_from(self.start) as usize;
		let layout = layout_u8(size_in_bytes);
		Global.dealloc(NonNull::new(self.start as *mut u8).unwrap(), layout);
	}

    /// Free a blob if it is unused. A blob is unused if the following criteria are met:
    /// * The blob is empty
    /// * The blob is not the current threadlocal blob.
    pub fn check_free(self) {
        if !self.is_empty() {
            return;
        }
        THREAD.with(|current| {
            if let Some(current) = current.read() {
                if current.start == self.start {
                    return;
                }
            }
        });
    }
}

fn layout_u8(size_in_bytes: usize) -> Layout {
    Layout::new::<u8>().repeat(size_in_bytes).unwrap().0
}