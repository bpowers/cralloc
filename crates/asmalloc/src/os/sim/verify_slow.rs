//! Kani proofs that the simulator's unsafe code is sound on the tiny shape, and that it keeps
//! the `Os` promises the allocator relies on: reserved memory reads as zero, commit keeps
//! contents, purge zeroes exactly its range, and an unmapped range comes back zero.
//!
//! The simulator has one region of memory and the ranges are two pages, so the loops over pages
//! stay short enough for CBMC. It still takes about a minute, so it is not in the merge gate;
//! run it with `scripts/kani-all verify_slow`.

use super::*;

/// Reserve two pages, commit them, write a byte anywhere in them, purge the first page, unmap
/// both and reserve again. Kani checks every memory access of the simulator along the way,
/// including the allocation, the zeroing and the free.
#[kani::proof]
#[kani::unwind(34)]
fn the_simulator_keeps_the_zero_promises() {
    let geometry = Tiny::GEOMETRY;
    let os = SimOs::<Tiny>::with_capacity(geometry.region_size());
    let page = geometry.page_size();

    let base = os.reserve(2 * page, page).unwrap();
    let offset: usize = kani::any();
    kani::assume(offset < 2 * page);
    let byte = base.map_addr(|a| a.saturating_add(offset)).as_ptr();
    let value: u8 = kani::any();
    kani::assume(value != 0);
    // SAFETY: each call's range is whole pages of the reservation, and nothing else uses it.
    unsafe {
        os.commit(base, 2 * page).unwrap();
        assert!(byte.read() == 0);
        byte.write(value);
        os.purge(base, page).unwrap();
        assert!(byte.read() == if offset < page { 0 } else { value });
        os.unmap(base, 2 * page).unwrap();
    }
    let again = os.reserve(2 * page, page).unwrap();
    assert!(again == base);
    // SAFETY: as above.
    unsafe {
        os.commit(again, 2 * page).unwrap();
        assert!(byte.read() == 0);
    }
}
