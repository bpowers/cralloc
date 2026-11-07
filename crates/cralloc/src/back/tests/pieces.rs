//! Purge steps bounded in pages whatever the unit (`purge::piece`): a large slot longer than
//! what a step may return goes back a piece a step, from its top down, the region's cursor
//! naming the page where the last piece began; what an allocation of a slot half returned
//! sees; and the heartbeat's step of the clocked pass, capped in pages.

use super::super::policy::{
    DECAY_TICKS, PURGE_STEP_PAGES, allowance_floor, growth_step, retained_cap,
};
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

/// Purges a step of at most `pages` pages from the region's cursor, or of a huge page in a
/// class of a huge page or more; the bytes it returned.
fn step<O: Os>(
    back: &Back<'_, O>,
    class: Class,
    region: crate::space::RegionId,
    pages: usize,
) -> usize {
    let page = Native::GEOMETRY.page_size();
    let huge = Native::GEOMETRY.huge_page_size();
    let (purged, _, _, _) = back.purge_region(
        class,
        region,
        usize::MAX,
        pages * page,
        true,
        Scope::All,
        false,
        true,
    );
    let most = if class.size() >= huge {
        (pages * page).max(huge)
    } else {
        pages * page
    };
    assert!(
        purged <= most,
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

/// A heartbeat's step of the clocked pass returns at most `PURGE_STEP_PAGES` pages, whatever the
/// bitmap it reads holds.
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

/// A carve that owes more growth than a step's pages pays at most a step's pages on its own
/// slow path; the rest stays owed and the next carve pays it.
#[test]
#[cfg_attr(miri, ignore)]
fn a_growth_payment_returns_at_most_a_steps_pages() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let freed = large(256 << 10);
    let size = freed.size();
    let n = 4 * retained_cap(Native::GEOMETRY) / size;
    let blocks: Vec<_> = (0..n)
        .map(|_| back.alloc(freed, size).unwrap().ptr)
        .collect();
    for b in &blocks {
        paint(b.as_ptr(), size, 7);
        back.free(*b, freed);
    }
    let cap = PURGE_STEP_PAGES as usize * Native::GEOMETRY.page_size();
    let big = large(700 << 10);
    assert!(big.size() > cap);
    let mut carved = Vec::new();
    let mut paid = 0;
    for _ in 0..3 {
        let before = back.retained_large();
        carved.push(back.alloc(big, big.size()).unwrap().ptr);
        let returned = before - back.retained_large();
        assert!(returned <= cap, "one carve paid {returned}");
        paid += returned;
    }
    assert!(
        paid > cap,
        "the debt a carve left was paid by the next: {paid}"
    );
    // What is still owed the heartbeats pay, a step each.
    let owing = || {
        back.state
            .growth
            .load(core::sync::atomic::Ordering::Relaxed)
    };
    assert!(
        owing() >= growth_step(Native::GEOMETRY),
        "the carves left {} owed",
        owing()
    );
    let mut beats = 0;
    while owing() >= growth_step(Native::GEOMETRY) {
        let before = back.retained_large();
        back.heartbeat();
        let returned = before - back.retained_large();
        assert!(
            returned > 0 && returned <= cap,
            "one heartbeat paid {returned}"
        );
        beats += 1;
        assert!(beats < 100);
    }
    for p in carved {
        back.free(p, big);
    }
}

/// The words a walk reads to get past live large slots when the step has one
/// page left to return.
#[test]
#[cfg_attr(miri, ignore)]
fn a_walk_passes_a_live_slot_at_a_look() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = large(256 << 10);
    let size = class.size();
    let page = Native::GEOMETRY.page_size();
    let low = back.alloc(class, size).unwrap().ptr;
    let region = back.space().locate(low.as_ptr()).unwrap().region;
    let mut live = Vec::new();
    loop {
        let b = back.alloc(class, size).unwrap().ptr;
        if back.space().locate(b.as_ptr()).unwrap().region != region {
            back.free(b, class);
            break;
        }
        paint(b.as_ptr(), size, 7);
        live.push(b);
    }
    paint(low.as_ptr(), size, 7);
    back.free(low, class);
    let (purged, read, _, _) = back.purge_region(
        class,
        region,
        usize::MAX,
        page,
        false,
        Scope::All,
        false,
        true,
    );
    // A piece of a class of a huge page or more is a whole huge page at least.
    assert_eq!(purged, Native::GEOMETRY.huge_page_size());
    assert!(
        read <= 4 * (live.len() + 1),
        "{read} words to pass {} live slots of {} pages",
        live.len(),
        size / page
    );
}

/// Random allocations, writes, resizes in place, frees and purge steps of random
/// sizes over two large classes. A block handed out as zeroed must read as zero over its
/// whole room.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_zeroed_block_reads_as_zero_whatever_the_pieces() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let page = Native::GEOMETRY.page_size();
    let classes = [large(64 << 10), large(256 << 10)];
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut rand = |n: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n
    };
    // (pointer, class, size)
    let mut held: Vec<(NonNull<u8>, Class, usize)> = Vec::new();
    let mut regions = std::collections::BTreeSet::new();
    let (mut zeroed, mut pieces) = (0, 0);
    for round in 0..200_000 {
        match rand(10) {
            0..=2 if held.len() < 24 => {
                let class = classes[rand(2)];
                let size = class.size() - rand(class.size() / 2);
                let block = if rand(2) == 0 {
                    back.alloc(class, size)
                } else {
                    back.alloc_staggered(class, size)
                }
                .unwrap();
                let room = back.room(block.ptr.as_ptr()).unwrap();
                if block.zeroed {
                    zeroed += 1;
                    assert!(
                        painted(block.ptr.as_ptr(), room, 0),
                        "round {round}: a block of {class:?} handed out as zero is not"
                    );
                }
                assert!(block.len >= size && block.len <= room);
                paint(block.ptr.as_ptr(), block.len, 0xab);
                let region = back.space().locate(block.ptr.as_ptr()).unwrap().region;
                regions.insert((class.index(), region.index()));
                held.push((block.ptr, class, size));
            }
            3..=4 if !held.is_empty() => {
                let (ptr, class, _) = held.swap_remove(rand(held.len()));
                back.free(ptr, class);
            }
            5 if !held.is_empty() => {
                let at = rand(held.len());
                let (ptr, _, _) = held[at];
                let room = back.room(ptr.as_ptr()).unwrap();
                let size = 1 + rand(room);
                // SAFETY: the test holds the block and gives up what is past `size`.
                unsafe { back.resize_in_place(ptr, size) };
                let len = back.usable_size(ptr.as_ptr()).unwrap();
                assert!(len >= size && len <= room);
                paint(ptr.as_ptr(), len, 0xcd);
                held[at].2 = size;
            }
            6..=8 if !regions.is_empty() => {
                let &(class, region) = regions.iter().nth(rand(regions.len())).unwrap();
                let class = classes
                    .iter()
                    .copied()
                    .find(|c| c.index() == class)
                    .unwrap();
                let region = back.space().plan().region(region).unwrap();
                let limit = (1 + rand(40)) * page;
                let (purged, _, _, _) = back.purge_region(
                    class,
                    region,
                    1 + rand(64),
                    limit,
                    rand(4) != 0,
                    Scope::All,
                    false,
                    true,
                );
                pieces += usize::from(purged != 0);
            }
            _ => {
                os.advance_clock(DECAY_TICKS / 16);
                back.heartbeat();
            }
        }
    }
    assert!(
        zeroed > 100 && pieces > 1000,
        "{zeroed} zeroed, {pieces} pieces"
    );
    for (ptr, class, _) in held {
        back.free(ptr, class);
    }
}

/// In a class of a huge page or more, a piece starts at a huge-page boundary and is a huge page
/// at least, however few pages the step may return: a slot kept with its huge-page advice is
/// never left with a huge page partly returned.
#[test]
#[cfg_attr(miri, ignore)]
fn pieces_of_a_class_of_huge_pages_are_whole_huge_pages() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let huge = Native::GEOMETRY.huge_page_size();
    assert!(class.size() >= 2 * huge);
    let (low, high, region) = free_slot_below_a_live_one(&back, class);
    let base = back.space().region(region).addr().get();
    let mut pieces = 0;
    while back.retained_large() > 0 {
        let before = back.retained_large();
        let purged = step(&back, class, region, 1);
        assert!(purged == 0 || purged == huge, "a piece of {purged} bytes");
        pieces += usize::from(purged != 0);
        assert!(pieces <= class.size() / huge);
        // The slot's pages from its top down to the last piece's start are all returned, and
        // that start is a huge-page boundary.
        let left = before - purged;
        let end = low.addr().get() + class.size();
        let start = end - (class.size() - left.min(class.size()));
        assert_eq!((start - base) % huge, 0);
        // SAFETY: the slot is free and nothing else uses the simulated heap meanwhile.
        let gone = unsafe { os.is_purged(NonNull::new(start as *mut u8).unwrap(), end - start) };
        assert!(gone);
    }
    assert_eq!(pieces, class.size() / huge);
    back.free(high, class);
}

/// A tenant that takes a slot between pieces, staggered and smaller than the slot, then grows
/// in place and is freed: the walk carries on below its cursor, a later walk from the top takes
/// what the tenant wrote, and the slot is handed out as zero only once every page is back,
/// and then reads as zero over its whole room.
#[test]
#[cfg_attr(miri, ignore)]
fn a_staggered_tenant_between_pieces_leaves_no_false_zero() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = large(256 << 10);
    let size = class.size();
    let huge = Native::GEOMETRY.huge_page_size();
    let (low, high, region) = free_slot_below_a_live_one(&back, class);
    assert_eq!(step(&back, class, region, 1), huge);
    let tenant = back.alloc_staggered(class, size / 3).unwrap();
    assert!(!tenant.zeroed);
    assert_eq!(
        back.space().locate(tenant.ptr.as_ptr()).unwrap().region,
        region
    );
    paint(tenant.ptr.as_ptr(), tenant.len, 9);
    // SAFETY: the test holds the block; it grows within its room.
    unsafe { back.resize_in_place(tenant.ptr, size / 2) };
    let len = back.usable_size(tenant.ptr.as_ptr()).unwrap();
    paint(tenant.ptr.as_ptr(), len, 9);
    back.free(tenant.ptr, class);
    // Walks until nothing is retained, from the cursor and then from the top.
    let mut steps = 0;
    while back.retained_large() > 0 {
        step(&back, class, region, 1);
        steps += 1;
        assert!(steps < 100);
    }
    let again = back.alloc(class, size).unwrap();
    assert_eq!(again.ptr, low);
    assert!(again.zeroed, "every page is back");
    let room = back.room(again.ptr.as_ptr()).unwrap();
    assert!(painted(again.ptr.as_ptr(), room, 0));
    back.free(again.ptr, class);
    back.free(high, class);
}

/// A growth payment whose walk returns nothing, over a cap that free slots on pages with live
/// neighbours hold up, leaves nothing owed: the heartbeats after it do not walk again.
#[test]
#[cfg_attr(miri, ignore)]
fn a_growth_payment_that_returns_nothing_leaves_nothing_owed() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let small = super::small(1024);
    let n = 4 * retained_cap(Native::GEOMETRY) / 1024;
    let objects = fill(&back, small, n);
    assert_eq!(objects.len(), n);
    // Every other object freed: every page keeps a live neighbour, so nothing can be purged.
    let freed: Vec<*mut u8> = objects.iter().step_by(2).copied().collect();
    let kept: Vec<*mut u8> = objects.iter().skip(1).step_by(2).copied().collect();
    // SAFETY: the test holds these objects and gives them up.
    unsafe { back.launder(small, &freed) };
    assert!(back.retained() > retained_cap(Native::GEOMETRY));
    let big = large(700 << 10);
    let block = back.alloc(big, big.size()).unwrap();
    let owing = back
        .state
        .growth
        .load(core::sync::atomic::Ordering::Relaxed);
    assert!(owing < growth_step(Native::GEOMETRY), "{owing} still owed");
    back.free(block.ptr, big);
    // SAFETY: as above.
    unsafe { back.launder(small, &kept) };
}
