use std::{
	cell::RefCell,
	ptr::null_mut,
};
use crate::blob::Blob;

pub struct Local {
    current: RefCell<*mut Blob>,
}

impl Local {
    fn read(&self) -> Option<Blob> {
        let ptr = self.current.borrow();
        if ptr.is_null() {
            None
        } else {
            Some(**ptr)
        }
    }

    fn write(&self, blob: Blob) {
        let current = self.read();
        let blob = Box::into_raw(Box::new(blob));
        *self.current.borrow_mut() = blob;
        if let Some(current) = current {
            current.check_free();
        }
    }
}

impl Default for Local {
	fn default() -> Self {
		Self {
            current: RefCell::new(null_mut()),
        }
	}
}

/// Free memory when the thread is dropped.
impl Drop for Local {
    fn drop(&mut self) {
        let current = self.read();
        if let Some(current) = current {
            *self.current.borrow_mut() = null_mut();
            current.check_free();
        }
    }
}

thread_local! {
    static THREAD: Local = Local::default();
}

pub fn with(f: impl FnOnce(&mut Local)) {
	THREAD.with(|thread| {
		let restore = thread.read();
	})
}
