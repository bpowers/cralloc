//! Purge steps bounded in pages whatever the unit (`purge::piece`): a large slot longer than
//! what a step may return goes back a piece a step, from its top down, the region's cursor
//! naming the page where the last piece began; what an allocation of a slot half returned
//! sees; and the heartbeat's step of the clocked pass, capped in pages.

use super::super::policy::{DECAY_TICKS, PURGE_STEP_PAGES, allowance_floor};
use super::super::purge::Scope;
use super::*;

/// A large slot of 64 pages, free and painted, below a live one in the same region. Returns
/// the free slot, the live one, and the region.
fn free_slot_below_a_live_one<O: Os>(
    back: &Back<'_, O>,
    class: Class,
) -> (NonNull<u8>, NonNull<u8>, crate::space::RegionId) {
    let size = class.size();
    let low = back.alloc(class, size).unwrap().ptr;
    let high = back.alloc(class, size).unwrap().ptr;
    let region = back.space().locate(low.as_ptr()).unwrap().region;
    assert_eq!(back.space().locate(high.as_ptr()).unwrap().region, region);
    paint(low.as_ptr(), size, 7);
    paint(high.as_ptr(), size, 7);
    back.free(low, class);
    (low, high, region)
}

/// Purges a step of at most `pages` pages from the region's cursor; the bytes it returned.
fn step<O: Os>(
    back: &Back<'_, O>,
    class: Class,
    region: crate::space::RegionId,
    pages: usize,
) -> usize {
    let page = Native::GEOMETRY.page_size();
    let (purged, _, _) =
        back.purge_region(class, region, usize::MAX, pages * page, true, Scope::All);
    assert!(
        purged <= pages * page,
        "a step of {pages} pages returned {purged} bytes"
    );
    purged
}

/// A slot four times a step's bound goes back in four steps, each within the bound, and once
/// every piece is back it reads as zero and is handed out zeroed.
#[test]
#[cfg_attr(miri, ignore)]
fn a_large_slot_goes_back_a_piece_a_step_and_comes_back_zeroed() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let size = class.size();
    let pages = size / Native::GEOMETRY.page_size() / 4;
    let (low, high, region) = free_slot_below_a_live_one(&back, class);
    let mut pieces = 0;
    while back.retained_large() > 0 {
        let purged = step(&back, class, region, pages);
        pieces += usize::from(purged != 0);
        assert!(pieces <= 4 + 1, "more steps than pieces");
    }
    assert_eq!(pieces, 4);
    // SAFETY: the slot is free and nothing else uses the simulated heap meanwhile.
    assert!(unsafe { os.is_purged(low, size) });
    let again = back.alloc(class, size).unwrap();
    assert_eq!(again.ptr, low);
    assert!(again.zeroed, "every piece went back: the slot is zero");
    assert!(painted(low.as_ptr(), size, 0));
    back.free(again.ptr, class);
    back.free(high, class);
}

/// A slot taken while only some of its pieces are back is not zero, and keeps what was not
/// returned; the pieces that were read as zero. Freed again, the walk carries on below.
#[test]
#[cfg_attr(miri, ignore)]
fn a_slot_taken_between_pieces_is_not_handed_out_zeroed() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = large(256 << 10);
    let size = class.size();
    let page = Native::GEOMETRY.page_size();
    let pages = size / page / 4;
    let (low, high, region) = free_slot_below_a_live_one(&back, class);
    let mut returned = 0;
    while returned < 2 * pages * page {
        returned += step(&back, class, region, pages);
    }
    let again = back.alloc(class, size).unwrap();
    assert_eq!(again.ptr, low);
    assert!(
        !again.zeroed,
        "half the slot is still as its last tenant left it"
    );
    let half = size / 2;
    assert!(painted(low.as_ptr(), half, 7));
    // SAFETY: inside the block just allocated.
    assert!(painted(unsafe { low.as_ptr().add(half) }, half, 0));
    // A step while it is held returns nothing of it.
    assert_eq!(step(&back, class, region, pages), 0);
    back.free(again.ptr, class);
    back.free(high, class);
}

/// A heartbeat's step of the clocked pass returns at most `PURGE_STEP_PAGES` pages, where it
/// used to be bounded only by the bitmap it read.
#[test]
#[cfg_attr(miri, ignore)]
fn a_heartbeats_step_of_the_clocked_pass_is_capped_in_pages() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let size = class.size();
    let n = 4 * allowance_floor(Native::GEOMETRY) / size;
    let blocks: Vec<_> = (0..n)
        .map(|_| back.alloc(class, size).unwrap().ptr)
        .collect();
    let top = back.alloc(class, size).unwrap().ptr;
    for b in &blocks {
        paint(b.as_ptr(), size, 7);
        back.free(*b, class);
    }
    let cap = PURGE_STEP_PAGES as usize * Native::GEOMETRY.page_size();
    let (mut largest, mut total) = (0, 0);
    // An interval apart, so that each heartbeat may start a pass, and the claims' frontiers
    // are two intervals old by the third; few enough that the hold's 64 fine steps never pass.
    for _ in 0..8 {
        os.advance_clock(DECAY_TICKS);
        let before = back.retained_large();
        back.heartbeat();
        let returned = before - back.retained_large();
        largest = largest.max(returned);
        total += returned;
    }
    assert!(total > cap, "the clocked pass returned {total}");
    assert!(largest <= cap, "one heartbeat returned {largest}");
    back.free(top, class);
}
