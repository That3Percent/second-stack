use rand::{
    distributions::{Distribution, Standard},
    rngs::StdRng,
    thread_rng, Rng, SeedableRng,
};
use second_stack::*;
use std::{fmt::Debug, marker::PhantomData, mem::MaybeUninit, thread};
use testdrop::TestDrop;

/// Randomly tests both uninit_slice and buffer
/// Includes tricky cases like recursing during iteration and drop
#[test]
fn soak() {
    let mut handles = Vec::new();

    for _ in 0..60 {
        let handle = thread::spawn(|| {
            for _ in 0..5 {
                recurse(8);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }
}

// This had to be separated out into a method because of this bizare
// compilation error:
// https://play.rust-lang.org/?version=stable&mode=debug&edition=2021&gist=0701319e1202978381ccba57e443a8b4
// Apparently what's going on here is that constrants shadow blanket impls.
fn get_seed() -> <StdRng as SeedableRng>::Seed {
    thread_rng().gen()
}

/// Grabs a randomly sized slice, verifies it's len, writes
/// random values to it, calls external function,
/// and verifies that all of the writes remained intact.
fn check_slice<T>(limit: u32)
where
    T: PartialEq + Debug,
    Standard: Distribution<T>,
{
    let len = thread_rng().gen_range(0usize..1025);

    let mut entered = false;

    uninit_slice(len, |uninit| {
        entered = true;
        let seed = get_seed();
        let mut rng = StdRng::from_seed(seed);

        assert_eq!(len, uninit.len());
        for i in 0..uninit.len() {
            let value = rng.gen();
            uninit[i] = MaybeUninit::new(value);
        }
        recurse(limit);
        let init = unsafe { &*(uninit as *const [MaybeUninit<T>] as *const [T]) };
        // Verify that nothing overwrote this array.
        let mut rng = StdRng::from_seed(seed);
        for i in 0..init.len() {
            let value = rng.gen();
            assert_eq!(init[i], value);
        }
    });
    assert!(entered);
}

fn rand_bool() -> bool {
    thread_rng().gen()
}

fn check_rand_method<T>(limit: u32)
where
    T: Debug + PartialEq,
    Standard: Distribution<T>,
{
    if rand_bool() {
        check_slice::<T>(limit);
    } else {
        check_iter::<T>(limit);
    }
}

fn check_rand_type(limit: u32) {
    let switch = thread_rng().gen_range(0u32..12);
    // Pick some types with varying size/alignment requirements
    match switch {
        0 => check_rand_method::<u8>(limit),
        1 => check_rand_method::<u16>(limit),
        2 => check_rand_method::<u32>(limit),
        3 => check_rand_method::<(u8, u8)>(limit),
        4 => check_rand_method::<(u8, u16)>(limit),
        5 => check_rand_method::<(u8, u32)>(limit),
        6 => check_rand_method::<(u16, u8)>(limit),
        7 => check_rand_method::<(u16, u16)>(limit),
        8 => check_rand_method::<(u16, u32)>(limit),
        9 => check_rand_method::<(u32, u8)>(limit),
        10 => check_rand_method::<(u32, u16)>(limit),
        11 => check_rand_method::<(u32, u32)>(limit),
        _ => unreachable!(),
    }
}

fn recurse(limit: u32) {
    if limit == 0 {
        return;
    }

    if thread_rng().gen() {
        check_rand_type(limit - 1);
    }
    if thread_rng().gen() {
        check_rand_type(limit - 1);
    }
}

fn check_iter<T>(limit: u32)
where
    T: Debug + PartialEq,
    Standard: Distribution<T>,
{
    let seed = get_seed();
    let total = thread_rng().gen_range(0..1025);
    let td = TestDrop::new();
    let iter: RandIterator<T> = RandIterator {
        total,
        count: 0,
        rand: StdRng::from_seed(seed),
        limit,
        drop: &td,
        _marker: PhantomData,
    };

    let mut check = CallCheck::new();
    buffer(iter, |items| {
        check.ok();
        assert_eq!(items.len(), total);
        let mut rand = StdRng::from_seed(seed);
        for item in items {
            assert_eq!(&item.value, &rand.gen());
        }
        recurse(limit);
    });

    assert_eq!(td.num_dropped_items(), td.num_tracked_items());
}

struct DropCheck<'a, T> {
    _item: testdrop::Item<'a>,
    limit: u32,
    probability: usize,
    value: T,
}

impl<T> Drop for DropCheck<'_, T> {
    fn drop(&mut self) {
        if thread_rng().gen_range(0..self.probability) == 0 {
            recurse(self.limit);
        }
    }
}

struct RandIterator<'a, T> {
    total: usize,
    count: usize,
    rand: StdRng,
    limit: u32,
    drop: &'a TestDrop,
    _marker: PhantomData<*const T>,
}

impl<'a, T> Iterator for RandIterator<'a, T>
where
    Standard: Distribution<T>,
{
    type Item = DropCheck<'a, T>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.total == self.count {
            return None;
        }
        let probability = self.total * 2;

        if thread_rng().gen_range(0..probability) == 0 {
            recurse(self.limit);
        }

        self.count += 1;
        let value = self.rand.gen();
        let item = self.drop.new_item().1;

        return Some(DropCheck {
            value,
            _item: item,
            probability,
            limit: self.limit,
        });
    }
}

struct CallCheck {
    called: bool,
}

impl CallCheck {
    pub fn new() -> Self {
        Self { called: false }
    }
    pub fn ok(&mut self) {
        self.called = true;
    }
}
impl Drop for CallCheck {
    #[track_caller]
    fn drop(&mut self) {
        assert!(self.called == true);
    }
}
