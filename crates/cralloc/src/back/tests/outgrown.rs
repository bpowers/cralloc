//! The slots blocks outgrow (`Back::move_large`, `Back::keep_outgrown`): kept within their
//! bound and returned past it, counted exactly through every way a kept slot leaves, and the
//! frees that are not outgrown slots.

use super::super::policy::{FINE_TICKS, HOLD_STEPS, allowance_floor};
use super::super::region;
use super::super::tree::{self, Host};
use super::super::word::{BITS, Word};
use super::*;
use crate::back::Move;

/// Heartbeats `n` times, the clock moving `ticks` before each.
fn beat_for<O: Os>(back: &Back<'_, O>, os: &SimOs<Native>, n: u32, ticks: u64) {
    for _ in 0..n {
        os.advance_clock(ticks);
        back.heartbeat();
    }
}

/// Moves `n` blocks of `len` bytes, each out of a fresh slot of `from` and into a fresh one of
/// `to`, as blocks that outgrew their slots. Returns the slots left behind and the blocks.
fn outgrow<O: Os>(
    back: &Back<'_, O>,
    from: Class,
    to: Class,
    len: usize,
    n: usize,
) -> (Vec<NonNull<u8>>, Vec<NonNull<u8>>) {
    let olds: Vec<NonNull<u8>> = (0..n).map(|_| back.alloc(from, len).unwrap().ptr).collect();
    let news: Vec<NonNull<u8>> = (0..n).map(|_| back.alloc(to, len).unwrap().ptr).collect();
    for (&old, &new) in olds.iter().zip(&news) {
        pattern(old.as_ptr(), len);
        // SAFETY: both are live blocks that nothing else uses, `old` given up.
        unsafe { back.move_large(old, new, len, false, Move::Outgrown) };
        assert!(patterned(new.as_ptr(), len));
    }
    (olds, news)
}

/// The slot `ptr` starts, with its class's host.
fn slot_of<'a, O: Os>(back: &Back<'a, O>, ptr: NonNull<u8>) -> (ClassHost<'a, O>, RegionId, u32) {
    let location = back.space().locate(ptr.as_ptr()).unwrap();
    let RegionTag::Class(class) = back.tag(location.region) else {
        panic!("not a class region");
    };
    let host = back.host(class).unwrap();
    let (slot, _) = host.layout().block_at(location.offset as usize).unwrap();
    (host, location.region, slot)
}

/// Whether the slot `ptr` starts is free, and marked as a kept outgrown slot.
fn kept<O: Os>(back: &Back<'_, O>, ptr: NonNull<u8>) -> bool {
    let (host, region, slot) = slot_of(back, ptr);
    let (word, bit) = (slot / BITS, slot % BITS);
    let free = tree::word(&host, tree::Node::leaf(region, word)).load() & (1u64 << bit) != 0;
    free && region::recorded(&host, region, slot).is_some_and(|r| r.outgrown)
}

/// What the count of kept outgrown slots should hold for the slots `slots`: the charge
/// (`region::charge`) of those that are free and marked.
fn owed<O: Os>(back: &Back<'_, O>, slots: &[NonNull<u8>]) -> usize {
    slots
        .iter()
        .filter(|&&p| kept(back, p))
        .map(|&p| {
            let (host, region, slot) = slot_of(back, p);
            region::charge(&host, region, slot)
        })
        .sum()
}

use super::super::host::ClassHost;
use crate::space::RegionId;

/// An outgrown slot is kept, resident, counted and stamped, while the count with it stays
/// within the bound, and returned behind the copy past it; a claim of a kept slot takes its
/// charge off the count, which makes room for the next.
#[test]
#[cfg_attr(miri, ignore)]
fn outgrown_slots_are_kept_within_their_bound_and_returned_past_it() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let bound = kept_outgrown(Native::GEOMETRY);
    let (from, to) = (large(192 << 10), large(256 << 10));
    let len = from.size() - 100;
    let fits = bound / from.size();
    beat_for(&back, os, 2, FINE_TICKS);
    let (olds, news) = outgrow(&back, from, to, len, fits + 2);
    for (i, &old) in olds.iter().enumerate() {
        if i < fits {
            assert!(kept(&back, old), "slot {i} is kept");
            assert!(patterned(old.as_ptr(), len), "slot {i} is resident");
            assert_ne!(back.hold_stamp_at(old.as_ptr()), 0, "slot {i} is stamped");
        } else {
            assert!(!kept(&back, old), "slot {i} is past the bound");
            // SAFETY: the slot is free and nothing else uses the simulated heap meanwhile.
            let purged = unsafe { os.is_purged(old, from.size()) };
            assert!(purged, "slot {i} went back");
        }
    }
    assert_eq!(back.outgrown(), fits * from.size());
    assert!(back.outgrown() <= bound);
    assert_eq!(back.outgrown(), owed(&back, &olds));
    // Every free large slot is a kept one, and all are counted as retained too.
    assert_eq!(back.retained_large(), back.outgrown());
    // A claim of a kept slot takes its charge off the count, and the next move is kept.
    let again = back.alloc(from, len).unwrap().ptr;
    assert_eq!(again, olds[0]);
    assert_eq!(back.outgrown(), (fits - 1) * from.size());
    // Its own claim takes the lowest kept slot, which it then outgrows and leaves kept again.
    let (more, more_news) = outgrow(&back, from, to, len, 1);
    assert_eq!(more[0], olds[1]);
    assert!(kept(&back, more[0]));
    assert_eq!(back.outgrown(), owed(&back, &olds));
    assert_eq!(back.outgrown(), (fits - 1) * from.size());
    back.free(again, from);
    for p in news.into_iter().chain(more_news) {
        back.free(p, to);
    }
    back.trim();
    assert_eq!(back.outgrown(), 0);
}

/// A slot that alone is past the bound is returned without the count ever holding it; a block
/// left mostly unwritten is never kept.
#[test]
#[cfg_attr(miri, ignore)]
fn a_slot_past_the_bound_alone_is_never_counted() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let bound = kept_outgrown(Native::GEOMETRY);
    // The largest slot of a region of this shape is half the bound; give the count all but
    // a page of room, so that any slot is past it alone.
    back.set_outgrown(bound - 4096);
    let (from, to) = (large(192 << 10), large(256 << 10));
    let (olds, news) = outgrow(&back, from, to, from.size(), 1);
    assert_eq!(back.outgrown(), bound - 4096, "refused and not counted");
    // SAFETY: the slot is free and nothing else uses the simulated heap meanwhile.
    assert!(unsafe { os.is_purged(olds[0], from.size()) });
    back.free(news[0], to);
}

/// The count is exact: it is the charge of the free slots marked kept, after a claim, a whole
/// purge by the hold, purges in pieces by the hold, a growth payment, the clocked pass and a
/// trim, and zero once every marked slot has been claimed or purged.
#[test]
#[cfg_attr(miri, ignore)]
fn the_count_is_exact_through_every_way_a_kept_slot_leaves() {
    let floor = allowance_floor(Native::GEOMETRY);
    // Whole slots for the hold (smaller than its bound a beat, a huge page here), and slots
    // the hold and the clocked pass return in pieces.
    for (from, to) in [
        (large(48 << 10), large(64 << 10)),
        (large(192 << 10), large(256 << 10)),
    ] {
        let state = state::<Native>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let len = from.size() - 100;
        beat_for(&back, os, 2, FINE_TICKS);
        // Over the hold's floor, which it returns down to, and within the bound.
        let n = (floor + floor / 2) / from.size();
        let (olds, news) = outgrow(&back, from, to, len, n);
        assert!(olds.iter().all(|&p| kept(&back, p)));
        assert_eq!(back.outgrown(), owed(&back, &olds));
        // A claim.
        let claimed = back.alloc(from, len).unwrap().ptr;
        assert!(!kept(&back, claimed));
        assert_eq!(back.outgrown(), owed(&back, &olds), "after a claim");
        back.free(claimed, from);
        assert_eq!(
            back.outgrown(),
            owed(&back, &olds),
            "a plain free adds nothing"
        );
        // The hold, a beat at a time: a whole slot, or a piece of one, each beat. A slot in
        // pieces is seen part returned between beats.
        let before = back.outgrown();
        let mut parted = false;
        for step in 0..HOLD_STEPS + 64 {
            os.advance_clock(FINE_TICKS);
            back.heartbeat();
            assert_eq!(back.outgrown(), owed(&back, &olds), "hold, step {step}");
            parted |= olds.iter().any(|&p| {
                let (host, region, slot) = slot_of(&back, p);
                let charge = region::charge(&host, region, slot);
                kept(&back, p) && charge != 0 && charge < from.size()
            });
        }
        assert!(back.outgrown() < before, "the hold returned some");
        if from.size() > super::super::policy::hold_beat(Native::GEOMETRY) {
            assert!(parted, "{} KiB slots go back in pieces", from.size() >> 10);
        }
        // A growth payment: another class carves past the cap, with plain frees to take it
        // over, and pays from wherever the walk stands, kept slots or not.
        let ballast = large(64 << 10);
        let blocks: Vec<NonNull<u8>> = (0..2 * retained_cap(Native::GEOMETRY) / ballast.size())
            .map(|_| back.alloc(ballast, ballast.size()).unwrap().ptr)
            .collect();
        for &b in &blocks {
            paint(b.as_ptr(), ballast.size(), 3);
            back.free(b, ballast);
        }
        let grower = large(96 << 10);
        let grown: Vec<NonNull<u8>> = (0..16)
            .map(|_| back.alloc(grower, grower.size()).unwrap().ptr)
            .collect();
        assert_eq!(back.outgrown(), owed(&back, &olds), "after growth paid");
        // The clocked pass.
        forget_reuse(&back);
        os.advance_clock(DECAY_TICKS);
        back.decay_pass(&|| back.now());
        assert_eq!(back.outgrown(), owed(&back, &olds), "after a clocked pass");
        for &p in &news {
            back.free(p, to);
        }
        for &p in &grown {
            back.free(p, grower);
        }
        back.trim();
        assert_eq!(back.outgrown(), 0, "a trim takes every kept slot");
        assert_eq!(owed(&back, &olds), 0);
    }
}

/// A grown block advised for huge pages returns its slot whole at its free and takes the
/// advice back; a grown block without advice is freed as any block is: resident, charged and
/// stamped. Neither is in the count of kept outgrown slots.
#[test]
#[cfg_attr(miri, ignore)]
fn a_grown_blocks_free_is_a_free() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    beat_for(&back, os, 2, FINE_TICKS);
    // Doubling past a huge page, densely: advised.
    let advised = back.alloc(large(512 << 10), huge).unwrap().ptr;
    move_in(&back, large(96 << 10), huge, advised, 2 * huge);
    assert_eq!(os.huge_pages(advised), Some(HugePages::Prefer));
    let (counted, retained) = (back.outgrown(), back.retained_large());
    back.free(advised, large(512 << 10));
    // SAFETY: the slot is free and nothing else uses the simulated heap meanwhile.
    assert!(unsafe { os.is_purged(advised, large(512 << 10).size()) });
    assert_eq!(os.huge_pages(advised), Some(HugePages::Avoid));
    assert_eq!(back.retained_large(), retained, "returned, not counted");
    assert_eq!(back.outgrown(), counted);
    // Grown by more than doubling: not advised, and freed as any block is.
    let plain = large(768 << 10);
    let block = back.alloc(plain, 3 * huge).unwrap().ptr;
    move_in(&back, large(80 << 10), huge, block, 3 * huge);
    assert_eq!(os.huge_pages(block), Some(HugePages::Avoid));
    paint(block.as_ptr(), 3 * huge, 4);
    // The slot `move_in` outgrew is kept; this block's free adds nothing to the count.
    let counted = back.outgrown();
    let retained = back.retained_large();
    let stamps = back.hold_stamp_at(block.as_ptr());
    os.advance_clock(FINE_TICKS);
    back.heartbeat();
    back.free(block, plain);
    assert!(painted(block.as_ptr(), 3 * huge, 4), "resident");
    let (host, region, slot) = slot_of(&back, block);
    assert_eq!(
        back.retained_large() - retained,
        region::charge(&host, region, slot)
    );
    assert_ne!(back.hold_stamp_at(block.as_ptr()), stamps, "stamped");
    assert_eq!(back.outgrown(), counted);
}

/// A move that is not an outgrowth, such as a shrink to a sliver of the slot, frees the old
/// slot as the block's free would: resident, charged and stamped, not in the count; and an
/// advised one goes back whole with its advice.
#[test]
#[cfg_attr(miri, ignore)]
fn a_shrinks_move_frees_the_old_slot_as_its_free_would() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    beat_for(&back, os, 2, FINE_TICKS);
    let class = large(768 << 10);
    let big = back.alloc(class, class.size()).unwrap().ptr;
    paint(big.as_ptr(), class.size(), 6);
    let sliver = large(48 << 10);
    let to = back.alloc(sliver, 40 << 10).unwrap().ptr;
    let (retained, purges) = (back.retained_large(), os.calls(Call::Purge));
    let stamp = back.hold_stamp_at(big.as_ptr());
    os.advance_clock(FINE_TICKS);
    back.heartbeat();
    // SAFETY: both are live blocks that nothing else uses, `big` given up.
    unsafe { back.move_large(big, to, 40 << 10, false, Move::Other) };
    assert!(painted(to.as_ptr(), 40 << 10, 6));
    assert_eq!(
        os.calls(Call::Purge),
        purges,
        "a copy of one piece returns nothing"
    );
    assert!(
        painted(big.as_ptr(), class.size(), 6),
        "the old slot stays resident"
    );
    assert_eq!(
        back.retained_large() - retained,
        class.size(),
        "charged as a free is"
    );
    assert_ne!(back.hold_stamp_at(big.as_ptr()), stamp, "and stamped");
    assert_eq!(back.outgrown(), 0, "and not kept as outgrown");
    // Advised: returned whole with its advice, as its free would.
    let advised = back.alloc(large(512 << 10), huge).unwrap().ptr;
    move_in(&back, large(96 << 10), huge, advised, 2 * huge);
    assert_eq!(os.huge_pages(advised), Some(HugePages::Prefer));
    let counted = back.outgrown();
    let to2 = back.alloc(sliver, 40 << 10).unwrap().ptr;
    // SAFETY: as above.
    unsafe { back.move_large(advised, to2, 40 << 10, false, Move::Other) };
    // SAFETY: the slot is free and nothing else uses the simulated heap meanwhile.
    assert!(unsafe { os.is_purged(advised, large(512 << 10).size()) });
    assert_eq!(os.huge_pages(advised), Some(HugePages::Avoid));
    assert_eq!(back.outgrown(), counted);
    back.free(to, sliver);
    back.free(to2, sliver);
}

/// A block advised for huge pages that outgrows its slot returns it behind the copy, with the
/// advice taken back, whatever room the count has: its pages fault back whole, and kept they
/// cost the next block more than the faults they save.
#[test]
#[cfg_attr(miri, ignore)]
fn an_advised_slot_outgrown_goes_back() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let class = large(512 << 10);
    let advised = back.alloc(class, huge).unwrap().ptr;
    move_in(&back, large(96 << 10), huge, advised, 2 * huge);
    assert_eq!(os.huge_pages(advised), Some(HugePages::Prefer));
    let (counted, retained) = (back.outgrown(), back.retained_large());
    let wider = large(768 << 10);
    let to = back.alloc(wider, 2 * huge).unwrap().ptr;
    // SAFETY: both are live blocks that nothing else uses, `advised` given up.
    unsafe { back.move_large(advised, to, 2 * huge, false, Move::Outgrown) };
    assert!(patterned(to.as_ptr(), huge));
    // SAFETY: the slot is free and nothing else uses the simulated heap meanwhile.
    assert!(unsafe { os.is_purged(advised, class.size()) });
    assert_eq!(os.huge_pages(advised), Some(HugePages::Avoid));
    assert!(!kept(&back, advised));
    assert_eq!(
        (back.outgrown(), back.retained_large()),
        (counted, retained)
    );
    back.free(to, wider);
}

/// The pages of `len` bytes at `at` that the simulated OS holds resident, in bytes, page by
/// page: what the OS says, not the clean bits.
fn resident_in(os: &SimOs<Native>, at: NonNull<u8>, len: usize) -> usize {
    let page = Native::GEOMETRY.page_size();
    (0..len / page)
        .filter(|i| {
            let p = at.map_addr(|a| a.saturating_add(i * page));
            // SAFETY: a page of a free slot; nothing else uses the simulated heap meanwhile.
            !unsafe { os.is_purged(p, page) }
        })
        .count()
        * page
}

/// A keep that brings the count exactly to its bound is within it.
#[test]
#[cfg_attr(miri, ignore)]
fn a_keep_that_reaches_the_bound_exactly_is_kept() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let bound = kept_outgrown(Native::GEOMETRY);
    let (from, to) = (large(192 << 10), large(256 << 10));
    // A block of the whole class, not staggered: its charge is the class.
    back.set_outgrown(bound - from.size());
    let (olds, news) = outgrow(&back, from, to, from.size(), 1);
    assert!(kept(&back, olds[0]), "kept at the bound");
    assert_eq!(back.outgrown(), bound);
    back.free(news[0], to);
}

/// The count alone decides whether an outgrown slot is kept, however many pieces its block
/// moves in. Within the bound a block of several pieces is kept whole, both copies resident
/// for the move, which the bound caps; past it the pages go back behind each piece, so the two
/// copies are never both resident beyond one piece, and the slot is not kept.
#[test]
#[cfg_attr(miri, ignore)]
fn a_block_of_several_pieces_is_kept_whole_within_the_bound_and_returned_piece_by_piece_past_it() {
    let piece = super::super::policy::move_piece(Native::GEOMETRY);
    let bound = kept_outgrown(Native::GEOMETRY);
    let (from, to) = (large(8 * piece), large(12 * piece));
    assert!(from.size() <= bound, "a slot the bound can hold");
    for past in [false, true] {
        let state = state::<Native>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let len = from.size();
        let old = back.alloc(from, len).unwrap().ptr;
        pattern(old.as_ptr(), len);
        let new = back.alloc(to, len).unwrap().ptr;
        if past {
            back.set_outgrown(bound);
        }
        let mut returned = Vec::new();
        // SAFETY: both are live blocks that nothing else uses, `old` given up.
        unsafe {
            back.move_large_with(old, new, len, false, Move::Outgrown, || {
                returned.push(len - resident_in(os, old, len));
            })
        };
        assert!(patterned(new.as_ptr(), len));
        assert!(returned.len() > 1, "a block of several pieces");
        for (i, &gone) in returned.iter().enumerate() {
            let want = if past { (i + 1) * piece } else { 0 };
            assert_eq!(gone, want, "past {past}: after piece {i}");
        }
        assert_eq!(kept(&back, old), !past);
        let resident = resident_in(os, old, len);
        assert_eq!(resident, if past { 0 } else { len });
        back.free(new, to);
    }
}

/// A move that is not an outgrowth returns the pieces behind its copy and frees the slot as
/// the block's free would: the free charges what the OS still holds of the slot, not the
/// pieces already gone.
#[test]
#[cfg_attr(miri, ignore)]
fn another_move_of_several_pieces_charges_only_what_stays_resident() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let piece = super::super::policy::move_piece(Native::GEOMETRY);
    let (from, to) = (large(8 * piece), large(12 * piece));
    let old = back.alloc(from, from.size()).unwrap().ptr;
    pattern(old.as_ptr(), from.size());
    let len = 5 * piece;
    let new = back.alloc(to, len).unwrap().ptr;
    let retained = back.retained_large();
    // SAFETY: both are live blocks that nothing else uses, `old` given up.
    unsafe { back.move_large(old, new, len, false, Move::Other) };
    assert!(patterned(new.as_ptr(), len));
    let resident = resident_in(os, old, from.size());
    assert_eq!(
        resident,
        from.size() - len,
        "the pieces behind the copy went back"
    );
    assert_eq!(
        back.retained_large() - retained,
        resident,
        "the free charges what stays resident"
    );
    assert!(!kept(&back, old));
    back.free(new, to);
}
