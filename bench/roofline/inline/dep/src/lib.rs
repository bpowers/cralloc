//! A dependency crate with one non-generic allocation site (compiled here) and one generic
//! one (instantiated in whichever crate uses it).
use std::hint::black_box;

#[inline(never)]
pub fn pair(n: u64) -> u64 {
    let mut acc = 0u64;
    for i in 0..n {
        let b = black_box(Box::new([i, 1, 2, 3]));
        acc = acc.wrapping_add(b[0]);
    }
    acc
}

#[inline(never)]
pub fn pair_generic<T: From<u64> + Into<u64> + Copy>(n: u64) -> u64 {
    let mut acc = 0u64;
    for i in 0..n {
        let b = black_box(Box::new(T::from(i)));
        acc = acc.wrapping_add((*b).into());
    }
    acc
}
