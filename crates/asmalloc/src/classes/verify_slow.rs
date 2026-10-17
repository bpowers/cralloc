//! Kani proofs too slow for the merge gate: about a minute or two each, run on demand with
//! `scripts/kani-all` or one at a time with `scripts/kani`.
//!
//! Each proves that the multiply-high in `divisor::quotient` divides exactly by one odd factor
//! for every input that can reach it. Kissat is several times faster than the default solver on
//! these multiplier problems.

use super::divisor::{MAGIC, quotient};

fn exact(q: u32) {
    let x: u32 = kani::any();
    kani::assume(x < 1 << 29);
    let index = quotient(x, MAGIC[(q >> 1) as usize]);
    let below = index * q;
    assert!(below <= x && x - below < q);
}

#[kani::proof]
#[kani::solver(kissat)]
fn the_quotient_by_3_is_exact() {
    exact(3);
}

#[kani::proof]
#[kani::solver(kissat)]
fn the_quotient_by_5_is_exact() {
    exact(5);
}

#[kani::proof]
#[kani::solver(kissat)]
fn the_quotient_by_7_is_exact() {
    exact(7);
}
