#[macro_use]
extern crate criterion;
use criterion::{Criterion, BenchmarkId};
use second_stack::{acquire};
#[cfg(feature = "experimental")]
use second_stack::{acquire_uninitialized};
use rand::prelude::*;
use rand_pcg::Pcg32;
use std::iter::{FromIterator, repeat};

// Much of this was copied from the soak test (with modifications)
// See also b574ca3e-d5e7-4691-bf1e-50f572a9a687
#[derive(Default, Clone)]
struct _64 { a: u64 }
#[derive(Default, Clone)]
struct _32 { a: u32 }
#[derive(Default, Clone)]
struct _16 { a: u16 }
#[derive(Default, Clone)]
struct _8 { a: u8 }
#[derive(Default, Clone)]
struct _64_32 { a: u64, b: u32 }
#[derive(Default, Clone)]
struct _64_16 { a: u64, b: u16 }
#[derive(Default, Clone)]
struct _64_8 { a: u64, b: u8 }

trait Alloc {
	fn alloc<T: 'static>(&self,
	i: impl Iterator<Item=T>) -> Box<dyn Drop>;
}

fn soak(allocator: impl Alloc, scale: usize) {
	fn recurse(i: usize, alloc: &impl Alloc, rng: &mut impl Rng, scale: usize) {
		if i < 4 { return; }
		let top = rng.gen_range(0, i);
		let bottom = i - top;

		let cap = rng.gen_range(1, i);
		let size = rng.gen_range(1, cap+1) * rng.gen_range(1, scale);
		let mode = rng.gen_range(0u8, 7u8);

		recurse(top, alloc, rng, scale);

		let dropper = match mode {
			0 => { alloc.alloc(repeat(_64::default()).take(size)) },
			1 => { alloc.alloc(repeat(_32::default()).take(size)) },
			2 => { alloc.alloc(repeat(_16::default()).take(size)) },
			3 => { alloc.alloc(repeat(_64_8::default()).take(size)) },
			4 => { alloc.alloc(repeat(_64_32::default()).take(size)) },
			5 => { alloc.alloc(repeat(_64_16::default()).take(size)) },
			6 => { alloc.alloc(repeat(_8::default()).take(size)) },
			_ => unreachable!(),
		};

		recurse(bottom, alloc, rng, scale);

		drop(dropper);
	}

	let mut rng = Pcg32::new(0xcafef00dd15ea5e5, 0xa02bdbf7bb3c0a7);

	for _ in 0..10 {
		recurse(768, &allocator, &mut rng, scale);
	}
}


fn initialized_comparison(c: &mut Criterion) {
	struct Acquire;
	impl Alloc for Acquire {
		fn alloc<T: 'static>(&self, i: impl Iterator<Item=T>) -> Box<dyn Drop> {
			Box::new(acquire(i))
		}
	}

	struct VecFromIter;
	impl Alloc for VecFromIter {
		fn alloc<T: 'static>(&self, i: impl Iterator<Item=T>) -> Box<dyn Drop> {
			Box::new(Vec::from_iter(i))
		}
	}

	let mut group = c.benchmark_group("initialized");
	for size in 4..11 {
		let size = 2 << size;
		group.bench_with_input(BenchmarkId::new("acquire", size), &size, move |b, i| {
			b.iter(|| soak(Acquire, *i))
		});
		group.bench_with_input(BenchmarkId::new("vec", size), &size, move |b, i| {
			b.iter(|| soak(VecFromIter, *i))
		});
	}
}

#[cfg(feature="experimental")]
fn uninitialized_comparison(c: &mut Criterion) {
	struct AcquireUninitialized;
	impl Alloc for AcquireUninitialized {
		fn alloc<T: 'static>(&self, i: impl Iterator<Item=T>) -> Box<dyn Drop> {
			Box::new(unsafe { acquire_uninitialized::<T>(i.size_hint().1.unwrap()) })
		}
	}

	struct VecWithCapacity;
	impl Alloc for VecWithCapacity {
		fn alloc<T: 'static>(&self, i: impl Iterator<Item=T>) -> Box<dyn Drop> {
			Box::new(Vec::<T>::with_capacity(i.size_hint().1.unwrap()))
		}
	}

	let mut group = c.benchmark_group("uninitialized");
	for size in 4..11 {
		let size = 2 << size;
		group.bench_with_input(BenchmarkId::new("acquire", size), &size, move |b, i| {
			b.iter(|| soak(AcquireUninitialized, *i))
		});
		group.bench_with_input(BenchmarkId::new("vec", size), &size, move |b, i| {
			b.iter(|| soak(VecWithCapacity, *i))
		});
	}
}

#[cfg(feature="experimental")]
criterion_group!(benches, initialized_comparison, uninitialized_comparison);
#[cfg(not(feature="experimental"))]
criterion_group!(benches, initialized_comparison);
criterion_main!(benches);