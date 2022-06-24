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
    let mut handles = Vec::with_capacity(64);

    for _ in 0..64 {
        let handle = thread::spawn(|| {
            for it in 0..500 {
                if thread_rng().gen_bool(0.002) {
                    dbg!(it);
                }
                thread::spawn(|| {
                    let local = Stack::new();
                    for _ in 0..5 {
                        recurse(12, &local);
                    }
                })
                .join()
                .unwrap();
            }
        });
        handles.push(handle);
    }

    for handle in handles.drain(..) {
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
fn check_slice<T>(limit: u32, local: &Stack)
where
    T: PartialEq + Debug,
    Standard: Distribution<T>,
{
    let len = thread_rng().gen_range(0usize..1025);

    let mut call_check = CallCheck::new();

    let f = move |uninit: &mut [MaybeUninit<T>]| {
        call_check.ok();
        let seed = get_seed();
        let mut rng = StdRng::from_seed(seed);

        assert_eq!(len, uninit.len());
        for i in 0..uninit.len() {
            let value = rng.gen();
            uninit[i] = MaybeUninit::new(value);
        }
        recurse(limit, local);
        let init = unsafe { &*(uninit as *const [MaybeUninit<T>] as *const [T]) };
        // Verify that nothing overwrote this array.
        let mut rng = StdRng::from_seed(seed);
        for i in 0..init.len() {
            let value = rng.gen();
            assert_eq!(init[i], value);
        }
    };

    if rand_bool() {
        uninit_slice(len, f);
    } else {
        local.uninit_slice(len, f)
    }
}

fn rand_bool() -> bool {
    thread_rng().gen()
}

fn check_rand_method<T>(limit: u32, local: &Stack)
where
    T: Debug + PartialEq,
    Standard: Distribution<T>,
{
    if rand_bool() {
        check_slice::<T>(limit, local);
    } else {
        check_iter::<T>(limit, local);
    }
}

fn check_rand_type(limit: u32, local: &Stack) {
    let switch = thread_rng().gen_range(0u32..13);
    // Pick some types with varying size/alignment requirements
    match switch {
        0 => check_rand_method::<u8>(limit, local),
        1 => check_rand_method::<u16>(limit, local),
        2 => check_rand_method::<u32>(limit, local),
        3 => check_rand_method::<(u8, u8)>(limit, local),
        4 => check_rand_method::<(u8, u16)>(limit, local),
        5 => check_rand_method::<(u8, u32)>(limit, local),
        6 => check_rand_method::<(u16, u8)>(limit, local),
        7 => check_rand_method::<(u16, u16)>(limit, local),
        8 => check_rand_method::<(u16, u32)>(limit, local),
        9 => check_rand_method::<(u32, u8)>(limit, local),
        10 => check_rand_method::<(u32, u16)>(limit, local),
        11 => check_rand_method::<(u32, u32)>(limit, local),
        12 => check_rand_method::<()>(limit, local),
        _ => unreachable!(),
    }
}

fn recurse(mut limit: u32, local: &Stack) {
    if limit == 0 {
        return;
    }

    limit -= 1;

    let with_local = |limit: u32, local: &Stack| {
        if thread_rng().gen() {
            check_rand_type(limit, local);
        }
        if thread_rng().gen() {
            check_rand_type(limit, local);
        }
    };

    if thread_rng().gen_range(0..8) == 0 {
        let new_local = Stack::new();
        with_local(limit, &new_local);
    } else {
        with_local(limit, local);
    }
}

fn check_iter<T>(limit: u32, local: &Stack)
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
        local,
        drop: &td,
        _marker: PhantomData,
    };

    let mut check = CallCheck::new();
    let f = |items: &mut [DropCheck<T>]| {
        check.ok();
        assert_eq!(items.len(), total);
        let mut rand = StdRng::from_seed(seed);
        for item in items {
            assert_eq!(&item.value, &rand.gen());
        }
        recurse(limit, local);
    };
    if rand_bool() {
        buffer(iter, f);
    } else {
        local.buffer(iter, f);
    }

    assert_eq!(td.num_dropped_items(), td.num_tracked_items());
}

struct DropCheck<'a, T> {
    _item: testdrop::Item<'a>,
    local: &'a Stack,
    limit: u32,
    probability: usize,
    value: T,
}

impl<T> Drop for DropCheck<'_, T> {
    fn drop(&mut self) {
        if thread_rng().gen_range(0..self.probability) == 0 {
            recurse(self.limit, self.local);
        }
    }
}

struct RandIterator<'a, T> {
    total: usize,
    count: usize,
    rand: StdRng,
    limit: u32,
    drop: &'a TestDrop,
    local: &'a Stack,
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
            recurse(self.limit, self.local);
        }

        self.count += 1;
        let value = self.rand.gen();
        let item = self.drop.new_item().1;

        return Some(DropCheck {
            value,
            _item: item,
            probability,
            local: self.local,
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
