#![feature(allocator_api, alloc_layout_extra, ptr_offset_from)]

mod blob;
mod local;

#[cfg(test)]
mod tests;

use local::with;
use std::mem::MaybeUninit;

pub fn acquire<T>(i: impl Iterator<Item = T>, f: impl FnOnce(&[T])) {
    with(|thread| {
		let blob = thread.read();
		let restore = blob.
        todo!();
        // TODO: Remember when growing that something else may have actually already grown the list down the stack.
        // TODO: Don't forget to drop the slice contents
    });
}

pub fn acquire_uninit<T>(size: usize, f: impl FnOnce(&[MaybeUninit<T>])) {
    with(|thread| {
		let blob = thread.read();
		todo!();
    });
}
