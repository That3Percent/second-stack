use rand::{
    distributions::{Distribution, Standard},
    rngs::StdRng,
    thread_rng, Rng, SeedableRng,
};
use second_stack::*;
use std::{fmt::Debug, mem::MaybeUninit, thread};

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
fn default_checks<F, T>(f: F)
where
    T: PartialEq + Debug,
    F: FnOnce(),
    Standard: Distribution<T>,
{
    let len = thread_rng().gen_range(0usize..1025);

    uninit_slice(len, |uninit| {
        let seed = get_seed();
        let mut rng = StdRng::from_seed(seed);

        assert_eq!(len, uninit.len());
        for i in 0..uninit.len() {
            let value = rng.gen();
            uninit[i] = MaybeUninit::new(value);
        }
        f();
        let init = unsafe { &*(uninit as *const [MaybeUninit<T>] as *const [T]) };
        // Verify that nothing overwrote this array.
        let mut rng = StdRng::from_seed(seed);
        for i in 0..init.len() {
            let value = rng.gen();
            assert_eq!(init[i], value);
        }
    })
}

fn check_rand_type<F>(f: F)
where
    F: FnOnce(),
{
    let switch = thread_rng().gen_range(0u32..12);
    // Pick some types with varying size/alignment requirements
    match switch {
        0 => default_checks::<_, u8>(f),
        1 => default_checks::<_, u16>(f),
        2 => default_checks::<_, u32>(f),
        3 => default_checks::<_, (u8, u8)>(f),
        4 => default_checks::<_, (u8, u16)>(f),
        5 => default_checks::<_, (u8, u32)>(f),
        6 => default_checks::<_, (u16, u8)>(f),
        7 => default_checks::<_, (u16, u16)>(f),
        8 => default_checks::<_, (u16, u32)>(f),
        9 => default_checks::<_, (u32, u8)>(f),
        10 => default_checks::<_, (u32, u16)>(f),
        11 => default_checks::<_, (u32, u32)>(f),
        _ => unreachable!(),
    }
}

#[test]
fn recursive_alloc() {
    fn recurse(limit: u32) {
        if limit == 0 {
            return;
        }
        if thread_rng().gen() {
            check_rand_type(|| recurse(limit - 1));
        }
        if thread_rng().gen() {
            check_rand_type(|| recurse(limit - 1));
        }
    }

    let mut handles = Vec::new();

    for _ in 0..25 {
        let handle = thread::spawn(|| {
            for i in 0..25 {
                recurse(i);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }
}
