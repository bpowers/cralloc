//! The back end over the simulated OS: unit tests, fault injection, a shadow-model property test
//! and a threaded stress test.

use super::policy::{CAP_REARM, DECAY_TICKS, large_retained_cap, move_piece, retained_cap};
use super::{Back, Block, Kind, State};
use crate::classes::{Class, SmallClass};
use crate::os::sim::{Call, Faults, Native, Shape, SimOs, Standard};
use crate::os::{HugePages, Os};
use crate::space::RegionTag;
use core::ptr::NonNull;
use std::boxed::Box;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::thread;
use std::vec::Vec;

type Sim<S = Standard> = SimOs<S>;

fn state<S: Shape>() -> Box<State<Sim<S>>> {
    Box::new(State::new(Sim::<S>::new()))
}

fn small(size: usize) -> SmallClass {
    Class::of_size(size).unwrap().small().unwrap()
}

fn large(size: usize) -> Class {
    let class = Class::of_size(size).unwrap();
    assert!(!class.is_small());
    class
}

fn fill<O: Os>(back: &Back<'_, O>, class: SmallClass, n: usize) -> Vec<*mut u8> {
    let mut out = std::vec![core::ptr::null_mut(); n];
    let got = back.fill(class, &mut out);
    out.truncate(got);
    out
}

/// Writes `len` bytes of `byte` at `ptr`.
fn paint(ptr: *mut u8, len: usize, byte: u8) {
    // SAFETY: the tests only paint slots the back end handed out, whose memory is committed
    // and theirs.
    unsafe { ptr.write_bytes(byte, len) };
}

/// Whether the `len` bytes at `ptr` all equal `byte`.
fn painted(ptr: *const u8, len: usize, byte: u8) -> bool {
    // SAFETY: as in `paint`.
    unsafe { core::slice::from_raw_parts(ptr, len) }
        .iter()
        .all(|&b| b == byte)
}

#[test]
fn fill_hands_out_distinct_aligned_writable_slots_in_address_order() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(48);
    let slots = fill(&back, class, 100);
    assert_eq!(slots.len(), 100);
    for pair in slots.windows(2) {
        assert_eq!(
            pair[1].addr() - pair[0].addr(),
            48,
            "carved in address order"
        );
    }
    for &p in &slots {
        assert_eq!(p.addr() % class.class().align(), 0);
        assert_eq!(back.lookup(p), Some(Kind::Slot(class.class())));
        assert_eq!(back.usable_size(p), Some(48));
        assert!(
            state
                .os()
                .is_writable(core::ptr::NonNull::new(p).unwrap(), 48)
        );
        paint(p, 48, 0xa5);
    }
    assert_eq!(back.lookup(slots[0].wrapping_add(1)), None);
}

/// With 8-byte slots the leaf bitmap of a region fills pages of its own, ahead of the page the
/// header shares with the summary, so it is committed only as carves need it. Freeing slots
/// writes it; the simulator's memory is inaccessible until committed, so a write the carve did
/// not commit for kills the test.
#[test]
fn the_leaf_bitmap_is_committed_before_it_is_written() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(8);
    let mut slots = fill(&back, class, 3000);
    assert_eq!(slots.len(), 3000);
    back.launder(class, &mut slots);
    assert_eq!(fill(&back, class, 3000).len(), 3000);
    back.trim();
}

#[test]
fn launder_then_fill_reuses_the_lowest_free_slots_first() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(64);
    let slots = fill(&back, class, 200);
    let mut freed: Vec<*mut u8> = slots[10..60].iter().rev().copied().collect();
    back.launder(class, &mut freed);
    assert_eq!(back.retained(), 50 * 64);
    let again = fill(&back, class, 20);
    assert_eq!(again, slots[10..30].to_vec());
    assert_eq!(back.retained(), 30 * 64);
    // Freed slots are used up before any new ones are carved.
    let rest = fill(&back, class, 40);
    assert_eq!(&rest[..30], &slots[30..60]);
    assert_eq!(rest[30], slots[199].wrapping_add(64));
}

#[test]
#[should_panic(expected = "double free")]
fn a_double_free_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(16);
    let slots = fill(&back, class, 4);
    back.launder(class, &mut [slots[1]]);
    back.launder(class, &mut [slots[1]]);
}

#[test]
#[should_panic(expected = "double free")]
fn a_slot_twice_in_one_batch_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(16);
    let slots = fill(&back, class, 4);
    back.launder(class, &mut [slots[1], slots[2], slots[1]]);
}

#[test]
#[should_panic(expected = "not the start of a slot")]
fn freeing_an_interior_pointer_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(32);
    let slots = fill(&back, class, 4);
    back.launder(class, &mut [slots[1].wrapping_add(8)]);
}

#[test]
#[should_panic(expected = "wrong class")]
fn freeing_with_the_wrong_class_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let slots = fill(&back, small(32), 4);
    back.launder(small(64), &mut [slots[0]]);
}

#[test]
#[should_panic(expected = "never handed out")]
fn freeing_an_uncarved_slot_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(32);
    let slots = fill(&back, class, 4);
    back.launder(class, &mut [slots[3].wrapping_add(32)]);
}

#[test]
#[should_panic(expected = "outside the heap")]
fn freeing_a_foreign_pointer_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let mut local = 0u8;
    back.launder(small(8), &mut [&raw mut local]);
}

#[test]
fn regions_are_assigned_as_a_class_grows() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    // 1 KiB slots, a class that gets huge pages: enough for two full regions and a few in a
    // third.
    let class = small(1024);
    let per = back.space().plan().slots_per_region(class.class()) as usize;
    let slots = fill(&back, class, 2 * per + 4);
    assert_eq!(slots.len(), 2 * per + 4);
    let regions: BTreeSet<usize> = slots
        .iter()
        .map(|&p| back.space().locate(p).unwrap().region.index())
        .collect();
    assert_eq!(regions.len(), 3);
    for &r in &regions {
        let region = back.space().plan().region(r).unwrap();
        assert_eq!(back.table().get(region), RegionTag::Class(class.class()));
    }
    // A region after the first is advised for huge pages from the start.
    let last = back.space().region(
        back.space()
            .plan()
            .region(*regions.last().unwrap())
            .unwrap(),
    );
    assert_eq!(state.os().huge_pages(last), Some(HugePages::Prefer));
    let first = back.space().region(
        back.space()
            .plan()
            .region(*regions.first().unwrap())
            .unwrap(),
    );
    assert_eq!(state.os().huge_pages(first), Some(HugePages::Avoid));
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_region_switches_to_huge_pages_past_the_threshold() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = small(64);
    let huge = Native::GEOMETRY.huge_page_size();
    // Six huge pages' worth of 64-byte slots, a magazine at a time: the first commit after the
    // region passes four huge pages switches it.
    let mut slots = Vec::new();
    while slots.len() < 6 * huge / 64 {
        slots.extend(fill(&back, class, 254));
    }
    let region = back.space().locate(slots[0]).unwrap().region;
    let base = back.space().region(region);
    assert_eq!(state.os().huge_pages(base), Some(HugePages::Avoid));
    let end = base.map_addr(|a| a.saturating_add(back.space().plan().region_size() - 1));
    assert_eq!(state.os().huge_pages(end), Some(HugePages::Prefer));
}

#[test]
#[cfg_attr(miri, ignore)]
fn large_slots_say_when_they_are_zero() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = large(64 << 10);
    let Block { ptr, zeroed } = back.alloc(class, class.size()).unwrap();
    assert!(zeroed && painted(ptr.as_ptr(), class.size(), 0));
    paint(ptr.as_ptr(), class.size(), 7);
    back.free(ptr, class);
    assert_eq!(back.retained(), class.size());
    let again = back.alloc(class, class.size()).unwrap();
    assert_eq!(again.ptr, ptr);
    assert!(!again.zeroed);
    assert!(painted(ptr.as_ptr(), class.size(), 7));
    // A second slot, so the first is not the region's top: purging works from any page.
    let other = back.alloc(class, class.size()).unwrap();
    back.free(again.ptr, class);
    assert!(back.trim() >= class.size());
    assert_eq!(back.retained(), 0);
    let purged = back.alloc(class, class.size()).unwrap();
    assert_eq!(purged.ptr, ptr);
    assert!(purged.zeroed && painted(ptr.as_ptr(), class.size(), 0));
    back.free_by_address(purged.ptr);
    back.free_by_address(other.ptr);
}

/// Several large slots to a region (the native shape's 1 MiB regions hold sixteen 64 KiB
/// slots): freeing them past the cap purges them, top down, until the count is under it; the
/// pages leave the resident set; and the purged slots come back zeroed, with their zero bits
/// set and cleared exactly once.
#[test]
#[cfg_attr(miri, ignore)]
fn large_slots_past_the_cap_are_purged_and_come_back_zeroed() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(64 << 10);
    let cap = retained_cap(Native::GEOMETRY);
    let n = 3 * cap / class.size();
    let blocks: Vec<Block> = (0..n)
        .map(|_| back.alloc(class, class.size()).unwrap())
        .collect();
    assert!(blocks.iter().all(|b| b.zeroed));
    for b in &blocks {
        paint(b.ptr.as_ptr(), class.size(), 0x5a);
    }
    // SAFETY: no other thread runs, and the back end and this test store only integers, so
    // every byte is initialized.
    let resident = || unsafe { os.resident_bytes() };
    let before = resident();
    assert!(before >= n * class.size());
    for b in &blocks {
        back.free(b.ptr, class);
    }
    assert!(back.retained() <= cap, "{} > {cap}", back.retained());
    let returned = before - resident();
    assert!(returned >= (n * class.size()).saturating_sub(cap) - class.size());
    let mut zeroed = 0;
    let again: Vec<Block> = (0..n)
        .map(|_| back.alloc(class, class.size()).unwrap())
        .collect();
    for b in &again {
        if b.zeroed {
            zeroed += 1;
            assert!(painted(b.ptr.as_ptr(), class.size(), 0));
        } else {
            assert!(painted(b.ptr.as_ptr(), class.size(), 0x5a));
        }
    }
    assert_eq!(
        zeroed * class.size(),
        returned.next_multiple_of(class.size())
    );
    for b in again {
        back.free(b.ptr, class);
    }
    back.trim();
    assert_eq!(back.retained(), 0);
    let after = resident();
    assert!(after < before / 8, "trim leaves {after} resident");
}

/// A large object's size is recorded when it is allocated and resized, and its usable size is
/// that rounded up to a page, never the whole slot; a small object's is its class size.
#[test]
#[cfg_attr(miri, ignore)]
fn large_objects_report_their_recorded_size() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let page = Native::GEOMETRY.page_size();
    let class = large(64 << 10);
    let block = back.alloc(class, 50_000).unwrap();
    let p = block.ptr.as_ptr();
    assert_eq!(back.size(p), Some(50_000));
    assert_eq!(
        back.usable_size(p),
        Some(50_000usize.next_multiple_of(page))
    );
    back.resize(block.ptr, 60_001);
    assert_eq!(back.size(p), Some(60_001));
    assert_eq!(
        back.usable_size(p),
        Some(60_001usize.next_multiple_of(page))
    );
    back.resize(block.ptr, class.size());
    assert_eq!(back.usable_size(p), Some(class.size()));
    back.free(block.ptr, class);
    let again = back.alloc(class, 10).unwrap();
    assert_eq!(again.ptr, block.ptr);
    assert_eq!(
        back.usable_size(p),
        Some(page),
        "the new size, not the old one"
    );
    let region = back.space().plan().region_size();
    let run = back.alloc_run(2 * region + 5, 8).unwrap();
    assert_eq!(back.size(run.as_ptr()), Some(2 * region + 5));
    back.resize(run, 3 * region);
    assert_eq!(back.usable_size(run.as_ptr()), Some(3 * region));
    let small_slot = fill(&back, small(48), 1)[0];
    assert_eq!(back.usable_size(small_slot), Some(48));
    back.resize(core::ptr::NonNull::new(small_slot).unwrap(), 10);
    assert_eq!(back.size(small_slot), Some(48));
}

#[test]
#[should_panic(expected = "beyond the object's slot")]
fn a_resize_past_the_slot_aborts() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = large(64 << 10);
    let block = back.alloc(class, 1).unwrap();
    back.resize(block.ptr, class.size() + 1);
}

#[test]
fn runs_hold_objects_bigger_than_a_region() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let region = back.space().plan().region_size();
    let ptr = back.alloc_run(3 * region + 1, 8).unwrap();
    assert_eq!(back.lookup(ptr.as_ptr()), Some(Kind::Run(4)));
    assert_eq!(back.size(ptr.as_ptr()), Some(3 * region + 1));
    let page = back.space().plan().geometry().page_size();
    assert_eq!(back.usable_size(ptr.as_ptr()), Some(3 * region + page));
    assert!(state.os().is_writable(ptr, 4 * region));
    // A run is advised against huge pages, at both ends.
    let last = ptr.map_addr(|a| a.saturating_add(4 * region - 1));
    assert_eq!(state.os().huge_pages(ptr), Some(HugePages::Avoid));
    assert_eq!(state.os().huge_pages(last), Some(HugePages::Avoid));
    assert!(painted(ptr.as_ptr(), 4 * region, 0));
    paint(ptr.as_ptr(), 4 * region, 3);
    assert_eq!(back.lookup(ptr.as_ptr().wrapping_add(region)), None);
    let second = back.alloc_run(region, 8).unwrap();
    assert_ne!(second, ptr);
    back.free_run(ptr);
    let again = back.alloc_run(4 * region, 8).unwrap();
    assert!(
        painted(again.as_ptr(), 4 * region, 0),
        "a reused run reads as zero"
    );
    let aligned = back.alloc_run(region + 1, 16 * region).unwrap();
    assert_eq!(aligned.addr().get() % (16 * region), 0);
    for p in [second, again, aligned] {
        back.free_by_address(p);
    }
    for r in back.space().plan().regions() {
        assert_eq!(back.table().get(r), RegionTag::Unassigned);
    }
}

#[test]
#[should_panic(expected = "not the start of a run")]
fn freeing_a_run_tail_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let region = back.space().plan().region_size();
    let ptr = back.alloc_run(2 * region, 8).unwrap();
    back.free_run(ptr.map_addr(|a| a.saturating_add(region)));
}

#[test]
fn the_heap_runs_out_and_recovers() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    // 32 KiB slots, in every object region.
    let class = small(32 << 10);
    let plan = back.space().plan();
    let all = plan.slots_per_region(class.class()) as usize * plan.objects();
    let mut slots = fill(&back, class, all + 5);
    assert_eq!(slots.len(), all);
    assert!(fill(&back, class, 1).is_empty());
    assert!(back.alloc(class.class(), 1).is_none());
    assert!(back.alloc_run(1, 8).is_none());
    let mut some = slots.split_off(all - 3);
    back.launder(class, &mut some);
    assert_eq!(fill(&back, class, 5).len(), 3);
}

#[test]
fn a_refused_commit_gives_null_and_nothing_else() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(128);
    state.os().set_faults(Call::Commit, Faults::ALWAYS);
    assert!(fill(&back, class, 10).is_empty());
    assert!(back.alloc(large(64 << 10), 1).is_none());
    assert!(back.alloc_run(1, 8).is_none());
    state.os().set_faults(Call::Commit, Faults::NONE);
    let slots = fill(&back, class, 10);
    assert_eq!(slots.len(), 10);
    // Fail the commit a carve needs in the middle of a fill: the fill stops short, and the slots
    // it did hand out are good.
    state.os().set_faults(Call::Commit, Faults::once_after(0));
    let more = fill(&back, class, 2000);
    assert!(more.len() < 2000);
    for &p in slots.iter().chain(&more) {
        assert!(
            state
                .os()
                .is_writable(core::ptr::NonNull::new(p).unwrap(), 128)
        );
    }
    let rest = fill(&back, class, 10);
    assert_eq!(rest.len(), 10);
}

#[test]
fn refused_advice_changes_nothing() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    state.os().set_faults(Call::AdviseHuge, Faults::ALWAYS);
    assert_eq!(fill(&back, small(1024), 300).len(), 300);
    assert!(back.alloc_run(1, 8).is_some());
}

#[test]
fn a_refused_purge_is_skipped_and_retried() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(1024);
    let mut slots = fill(&back, class, 128);
    for &p in &slots {
        paint(p, 1024, 9);
    }
    let keep = slots.split_off(64);
    back.launder(class, &mut slots);
    state.os().set_faults(Call::Purge, Faults::ALWAYS);
    assert_eq!(back.trim(), 0);
    state.os().set_faults(Call::Purge, Faults::NONE);
    for &p in &keep {
        assert!(painted(p, 1024, 9), "a refused purge touched a live slot");
    }
    assert_eq!(back.trim(), 64 * 1024);
    for &p in &keep {
        assert!(painted(p, 1024, 9));
    }
    let again = fill(&back, class, 64);
    assert!(again.iter().all(|&p| painted(p, 1024, 0)));
}

#[test]
fn passing_the_cap_purges_back_under_it() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let cap = retained_cap(Standard::GEOMETRY);
    let class = small(1024);
    let n = 2 * cap / 1024;
    let mut slots = fill(&back, class, n);
    assert_eq!(slots.len(), n);
    let before = state.os().calls(Call::Purge);
    back.launder(class, &mut slots);
    assert!(state.os().calls(Call::Purge) > before);
    assert!(back.retained() <= cap, "{} > {cap}", back.retained());
    // The purged pages are the top ones: the lowest slots are still resident.
    let low = fill(&back, class, 1)[0];
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    let purged = unsafe {
        state
            .os()
            .is_purged(core::ptr::NonNull::new(low).unwrap(), 1024)
    };
    assert!(!purged);
}

/// How many heartbeats the tests give a decay pass to finish: far more than a pass over a
/// simulated heap needs.
const BEATS: usize = 4000;

fn beat<O: Os>(back: &Back<'_, O>) {
    for _ in 0..BEATS {
        back.heartbeat();
    }
}

fn pass_under_way<O: Os>(state: &State<O>) -> bool {
    state.decay.load(core::sync::atomic::Ordering::Relaxed) & 1 != 0
}

/// The defect the reset caused (`docs/research/memory.md`): in a storm of frees every page
/// still has a live neighbour while the cap's purge runs, so it purges nothing. The count must
/// keep those bytes, so that when the neighbours are freed too the memory is still known and
/// is returned. The old code zeroed the count after the fruitless pass; the second assertion
/// fails on it, and so does the last, because the survivors' bytes alone never reached the cap.
#[test]
#[cfg_attr(miri, ignore)]
fn a_fruitless_purge_does_not_make_the_count_forget() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let cap = retained_cap(Standard::GEOMETRY);
    let class = small(1024);
    let per_page = Standard::GEOMETRY.page_size() / 1024;
    let n = 2 * cap / 1024;
    let slots = fill(&back, class, n);
    assert_eq!(slots.len(), n);
    for &p in &slots {
        paint(p, 1024, 3);
    }
    // One slot of every page stays live; the rest are freed, well past the cap.
    let (mut survivors, mut storm): (Vec<*mut u8>, Vec<*mut u8>) = slots
        .iter()
        .partition(|p| (p.addr() / 1024).is_multiple_of(per_page));
    let freed = storm.len() * 1024;
    assert!(freed > cap && survivors.len() * 1024 < cap);
    let purges = os.calls(Call::Purge);
    for chunk in storm.chunks_mut(64) {
        back.launder(class, chunk);
    }
    assert_eq!(os.calls(Call::Purge), purges, "no page was wholly free");
    assert_eq!(
        back.retained_small(),
        freed,
        "the count forgot fragmented bytes"
    );
    // The neighbours go too. Nothing is live now, and the memory comes back with the clock.
    for chunk in survivors.chunks_mut(64) {
        back.launder(class, chunk);
    }
    // The count still knew the storm's bytes, so these frees took it over further steps and
    // the cap acted on pages that are wholly free now.
    assert!(os.calls(Call::Purge) > purges);
    assert!(back.retained_small() <= cap + cap / CAP_REARM);
    for _ in 0..40 {
        os.advance_clock(DECAY_TICKS);
        beat(&back);
    }
    assert_eq!(back.retained(), 0);
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    let resident = unsafe { os.resident_bytes() };
    assert!(resident < n * 1024 / 16, "{resident} bytes still resident");
}

/// Below the cap nothing is purged until an interval has passed. Then one pass returns about a
/// quarter, from the top; it does not run again until the next interval; and what stays free
/// is all gone after enough intervals.
#[test]
#[cfg_attr(miri, ignore)]
fn free_memory_decays_by_quarters_from_the_top() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = small(1024);
    let total = retained_cap(Standard::GEOMETRY) / 2;
    let mut slots = fill(&back, class, total / 1024);
    for &p in &slots {
        paint(p, 1024, 5);
    }
    let (lowest, highest) = (slots[0], slots[slots.len() - 1]);
    back.launder(class, &mut slots);
    assert_eq!(back.retained(), total);
    let purges = os.calls(Call::Purge);
    beat(&back);
    os.advance_clock(DECAY_TICKS - 1);
    beat(&back);
    assert_eq!(
        os.calls(Call::Purge),
        purges,
        "purged before an interval had passed"
    );
    os.advance_clock(1);
    beat(&back);
    assert!(!pass_under_way(&state));
    let left = back.retained();
    assert!(
        left <= total - total / 4 && left > total / 2,
        "{left} of {total}"
    );
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe {
        assert!(os.is_purged(NonNull::new(highest).unwrap(), 1024));
        assert!(!os.is_purged(NonNull::new(lowest).unwrap(), 1024));
    }
    // No second pass inside the same interval.
    let purges = os.calls(Call::Purge);
    beat(&back);
    assert_eq!(os.calls(Call::Purge), purges);
    for _ in 0..40 {
        os.advance_clock(DECAY_TICKS);
        beat(&back);
    }
    assert_eq!(back.retained(), 0);
}

/// Memory that is reused is not counted as retained and not purged: a claim lowers the count,
/// and a claim that touches a purged page counts the rest of that page as resident again.
#[test]
fn reuse_lowers_the_count_and_reviving_a_page_raises_it() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(1024);
    let page = Standard::GEOMETRY.page_size();
    let mut slots = fill(&back, class, 64);
    back.launder(class, &mut slots);
    assert_eq!(back.retained(), 64 * 1024);
    let again = fill(&back, class, 16);
    assert_eq!(back.retained(), 48 * 1024);
    assert_eq!(back.trim(), 48 * 1024);
    assert_eq!(back.retained(), 0);
    // One slot of a purged page: the page's other slots are free and resident again.
    let one = fill(&back, class, 1);
    assert_eq!(back.retained(), page - 1024);
    let mut all: Vec<*mut u8> = again.into_iter().chain(one).collect();
    back.launder(class, &mut all);
    assert_eq!(back.retained(), page + 16 * 1024);
}

/// A heap whose free memory is all fragmentation is walked once an interval and not between:
/// the pass ends after one lap, purges nothing, and leaves the count alone.
#[test]
#[cfg_attr(miri, ignore)]
fn a_fragmented_heap_costs_one_lap_an_interval() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = small(1024);
    let per_page = Standard::GEOMETRY.page_size() / 1024;
    let slots = fill(&back, class, 1024);
    let (_live, mut freed): (Vec<*mut u8>, Vec<*mut u8>) = slots
        .iter()
        .partition(|p| (p.addr() / 1024).is_multiple_of(per_page));
    back.launder(class, &mut freed);
    let retained = back.retained();
    let laps = || state.laps.load(core::sync::atomic::Ordering::Relaxed);
    for _ in 0..3 {
        os.advance_clock(DECAY_TICKS);
        let before = laps();
        back.heartbeat();
        assert!(pass_under_way(&state));
        beat(&back);
        assert!(!pass_under_way(&state));
        assert_eq!(laps(), before + 1, "a pass is one lap");
        beat(&back);
        assert_eq!(laps(), before + 1, "no walking between passes");
    }
    assert_eq!(os.calls(Call::Purge), 0);
    assert_eq!(back.retained(), retained);
}

/// While the count stays over the cap, a launder purges again each time the count has grown
/// by another quarter of the cap, so memory that becomes purgeable later in a burst is found
/// without waiting for the clock.
#[test]
#[cfg_attr(miri, ignore)]
fn the_cap_acts_again_as_the_count_grows_past_it() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let cap = retained_cap(Standard::GEOMETRY);
    let class = small(1024);
    let per_page = Standard::GEOMETRY.page_size() / 1024;
    let slots = fill(&back, class, 2 * cap / 1024);
    let (mut survivors, mut storm): (Vec<*mut u8>, Vec<*mut u8>) = slots
        .iter()
        .partition(|p| (p.addr() / 1024).is_multiple_of(per_page));
    for chunk in storm.chunks_mut(64) {
        back.launder(class, chunk);
    }
    assert_eq!(os.calls(Call::Purge), 0);
    assert!(back.retained_small() > cap);
    // Freeing the survivors makes pages wholly free, and takes the count over further steps.
    for chunk in survivors.chunks_mut(64) {
        back.launder(class, chunk);
    }
    assert!(os.calls(Call::Purge) > 0, "the cap never acted again");
    assert!(back.retained_small() <= cap + cap / CAP_REARM);
}

/// Freed large slots are kept up to their own small cap; past it each free returns the slot
/// it frees, so the count never passes the cap by more than nothing; and a program that frees
/// and reallocates one slot at the cap does not purge each time round.
#[test]
#[cfg_attr(miri, ignore)]
fn freed_large_slots_have_a_small_cap_of_their_own() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(64 << 10);
    let cap = large_retained_cap(Native::GEOMETRY);
    let n = 3 * cap / class.size();
    let blocks: Vec<Block> = (0..n)
        .map(|_| back.alloc(class, class.size()).unwrap())
        .collect();
    for b in &blocks {
        paint(b.ptr.as_ptr(), class.size(), 0x33);
    }
    for (i, b) in blocks.iter().enumerate() {
        back.free(b.ptr, class);
        assert!(back.retained_large() <= cap, "after {i} frees");
    }
    assert_eq!(back.retained_large(), cap);
    assert_eq!(back.retained_small(), 0);
    assert_eq!(os.calls(Call::Purge) as usize, n - cap / class.size());
    // The first freed were kept and the later ones purged.
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe {
        assert!(!os.is_purged(blocks[0].ptr, class.size()));
        assert!(os.is_purged(blocks[n - 1].ptr, class.size()));
    }
    let purges = os.calls(Call::Purge);
    for _ in 0..100 {
        let block = back.alloc(class, class.size()).unwrap();
        assert!(!block.zeroed, "reuse takes a kept slot");
        back.free(block.ptr, class);
    }
    assert_eq!(os.calls(Call::Purge), purges, "a loop at the cap purged");
    // A slot bigger than the cap goes back at once and comes back zeroed.
    let big = large(2 * cap);
    let block = back.alloc(big, big.size()).unwrap();
    paint(block.ptr.as_ptr(), big.size(), 1);
    back.free(block.ptr, big);
    assert_eq!(back.retained_large(), cap);
    let again = back.alloc(big, big.size()).unwrap();
    assert!(again.zeroed && painted(again.ptr.as_ptr(), big.size(), 0));
}

/// Only classes of small, dense slots are ever advised for huge pages, however much they
/// carve: a block of a page or more is often touched in part.
#[test]
#[cfg_attr(miri, ignore)]
fn classes_of_a_page_or_more_never_get_huge_pages() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let plan = back.space().plan();
    for class in [small(2048).class(), small(8 << 10).class(), large(64 << 10)] {
        // More than a region's worth, so a second region is assigned too.
        let n = plan.slots_per_region(class) as usize + 2;
        let blocks: Vec<Block> = (0..n)
            .map(|_| back.alloc(class, class.size()).unwrap())
            .collect();
        for b in [blocks[0], blocks[n - 1]] {
            let region = back.space().locate(b.ptr.as_ptr()).unwrap().region;
            let base = back.space().region(region);
            let end = base.map_addr(|a| a.saturating_add(plan.region_size() - 1));
            assert_eq!(
                state.os().huge_pages(base),
                Some(HugePages::Avoid),
                "{class:?}"
            );
            assert_eq!(
                state.os().huge_pages(end),
                Some(HugePages::Avoid),
                "{class:?}"
            );
        }
    }
}

/// The huge page that holds the watermark can be purged: the purge carves the rest of it,
/// releases those slots as free and counts them, and then takes the page like any other. The
/// carved slots are ordinary free slots afterwards.
#[test]
#[cfg_attr(miri, ignore)]
fn the_huge_page_at_the_watermark_is_purged_with_the_rest() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = small(64);
    let huge = Native::GEOMETRY.huge_page_size();
    // Past the threshold and half way into a huge page of the huge-page part.
    let mut slots = Vec::new();
    while slots.len() < (6 * huge + huge / 2) / 64 {
        slots.extend(fill(&back, class, 64));
    }
    let count = slots.len();
    let top = slots[count - 1];
    for &p in &slots {
        paint(p, 64, 8);
    }
    let region = back.space().locate(top).unwrap().region;
    let base = back.space().region(region).addr().get();
    let unit = (top.addr() - base) / huge;
    assert!(
        !(top.addr() + 64 - base).is_multiple_of(huge),
        "the watermark is mid-page"
    );
    for chunk in slots.chunks_mut(64) {
        back.launder(class, chunk);
    }
    let purged = back.trim();
    // Everything carved by the program, and the rest of the watermark's huge page.
    assert_eq!(purged, (unit + 1) * huge);
    assert_eq!(back.retained(), 0);
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe {
        assert!(os.is_purged(NonNull::new(top).unwrap(), 64));
    }
    // The slots carved to fill the page are handed out like any others, each once.
    let again = fill(&back, class, (unit + 1) * huge / 64);
    let distinct: BTreeSet<usize> = again.iter().map(|p| p.addr()).collect();
    assert_eq!(distinct.len(), (unit + 1) * huge / 64);
    assert!(again.iter().all(|&p| p.addr() < base + (unit + 1) * huge));
    assert!(again.iter().all(|&p| painted(p, 64, 0)));
}

/// A watermark page with a live slot below the watermark is left alone: nothing is carved.
#[test]
#[cfg_attr(miri, ignore)]
fn a_watermark_page_in_use_is_not_carved() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = small(64);
    let huge = Native::GEOMETRY.huge_page_size();
    let mut slots = Vec::new();
    while slots.len() < (6 * huge + huge / 2) / 64 {
        slots.extend(fill(&back, class, 64));
    }
    let count = slots.len();
    let live = slots.pop().unwrap();
    for chunk in slots.chunks_mut(64) {
        back.launder(class, chunk);
    }
    back.trim();
    // The next never-used slot is still the one after the last the program got.
    let mut all = fill(&back, class, count);
    all.sort_unstable();
    assert_eq!(all[count - 1].addr(), live.addr() + 64);
}

/// A tiny random number generator, so the property tests need no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// What the shadow model knows about a live block.
#[derive(Clone, Copy, Debug)]
struct Live {
    ptr: *mut u8,
    class: Option<Class>,
    len: usize,
    byte: u8,
}

/// Runs random operations against the back end and a shadow model: every block handed out is
/// in the heap, disjoint from every live block, as long as its class, keeps its contents until
/// freed, and reads as zero when the back end says so. With `faults`, commits, purges and advice
/// fail at random, and a failure may only show as a null result or a skipped purge.
fn shadow_run<S: Shape>(seed: u64, steps: usize, faults: bool) {
    let state = state::<S>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let mut rng = Rng(seed | 1);
    let smalls: Vec<SmallClass> = [8, 24, 48, 256, 1024, 8192]
        .into_iter()
        .map(small)
        .collect();
    let larges: Vec<Class> = Class::all()
        .filter(|&c| !c.is_small() && back.space().plan().fits_region(c))
        .take(4)
        .collect();
    let mut live: BTreeMap<usize, Live> = BTreeMap::new();
    let mut byte = 0u8;
    let region = back.space().plan().region_size();
    let add = |live: &mut BTreeMap<usize, Live>, ptr: *mut u8, entry: Live| {
        assert_eq!(ptr, entry.ptr);
        let start = ptr.addr();
        if let Some((&below, b)) = live.range(..=start).next_back() {
            assert!(below + b.len <= start, "overlaps the block below");
        }
        if let Some((&above, _)) = live.range(start..).next() {
            assert!(start + entry.len <= above, "overlaps the block above");
        }
        assert!(back.space().locate(ptr).is_some());
        if let Some(class) = entry.class {
            assert_eq!(start % class.align().min(region), 0);
        }
        paint(ptr, entry.len, entry.byte);
        live.insert(start, entry);
    };
    for _ in 0..steps {
        if faults && rng.below(8) == 0 {
            let call = [Call::Commit, Call::Purge, Call::AdviseHuge][rng.below(3)];
            let plan = match rng.below(3) {
                0 => Faults::NONE,
                1 => Faults::once_after(rng.below(4) as u32),
                _ => Faults {
                    pass: rng.below(3) as u32,
                    fail: rng.below(5) as u32,
                    // A failing call may first act on half its range, as Linux allows.
                    partial: rng.below(2) == 0,
                },
            };
            os.set_faults(call, plan);
        }
        byte = byte.wrapping_add(1).max(1);
        match rng.below(10) {
            0..=2 => {
                let class = smalls[rng.below(smalls.len())];
                for p in fill(&back, class, 1 + rng.below(40)) {
                    let entry = Live {
                        ptr: p,
                        class: Some(class.class()),
                        len: class.size(),
                        byte,
                    };
                    add(&mut live, p, entry);
                }
            }
            3 if !larges.is_empty() => {
                let class = larges[rng.below(larges.len())];
                if let Some(block) = back.alloc(class, class.size()) {
                    if block.zeroed {
                        assert!(painted(block.ptr.as_ptr(), class.size(), 0), "{class:?}");
                    }
                    let entry = Live {
                        ptr: block.ptr.as_ptr(),
                        class: Some(class),
                        len: class.size(),
                        byte,
                    };
                    add(&mut live, block.ptr.as_ptr(), entry);
                }
            }
            4 if rng.below(4) == 0 => {
                let len = 1 + rng.below(3 * region);
                if let Some(ptr) = back.alloc_run(len, 8) {
                    assert!(painted(ptr.as_ptr(), len, 0));
                    let regions = len.div_ceil(region);
                    assert_eq!(back.lookup(ptr.as_ptr()), Some(Kind::Run(regions)));
                    let entry = Live {
                        ptr: ptr.as_ptr(),
                        class: None,
                        len: regions * region,
                        byte,
                    };
                    add(&mut live, ptr.as_ptr(), entry);
                }
            }
            5..=7 if !live.is_empty() => {
                // Free a handful of blocks; small ones of one class go back as one batch.
                let keys: Vec<usize> = live.keys().copied().collect();
                let first = keys[rng.below(keys.len())];
                let class = live[&first].class;
                let mut batch: Vec<*mut u8> = std::vec![live[&first].ptr];
                if class.is_some_and(|c| c.is_small()) {
                    for &k in keys.iter().skip(rng.below(keys.len())).take(30) {
                        if k != first && live[&k].class == class {
                            batch.push(live[&k].ptr);
                        }
                    }
                }
                for &p in &batch {
                    let entry = live.remove(&p.addr()).unwrap();
                    assert!(painted(p, entry.len, entry.byte), "a live block changed");
                }
                match class {
                    Some(c) if c.is_small() => {
                        let mut batch = batch;
                        back.launder(c.small().unwrap(), &mut batch);
                    }
                    _ => {
                        for p in batch {
                            back.free_by_address(core::ptr::NonNull::new(p).unwrap());
                        }
                    }
                }
            }
            8 => back.heartbeat(),
            _ if rng.below(8) == 0 => {
                back.trim();
            }
            _ => {}
        }
    }
    for c in Call::ALL {
        os.set_faults(c, Faults::NONE);
    }
    for entry in live.values() {
        assert!(
            painted(entry.ptr, entry.len, entry.byte),
            "a live block changed"
        );
    }
}

#[test]
fn random_operations_match_the_shadow_model() {
    let (seeds, steps) = if cfg!(miri) { (1, 40) } else { (12, 1500) };
    for seed in 0..seeds {
        shadow_run::<Standard>(0x9e37_79b9_7f4a_7c15 ^ seed, steps, false);
    }
}

#[test]
fn random_operations_survive_os_failures() {
    let (seeds, steps) = if cfg!(miri) { (1, 40) } else { (12, 1500) };
    for seed in 0..seeds {
        shadow_run::<Standard>(0x2545_f491_4f6c_dd1d ^ seed, steps, true);
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn random_operations_with_several_large_slots_per_region() {
    for seed in 0..4 {
        shadow_run::<Native>(0x5851_f42d_4c95_7f2d ^ seed, 1500, seed % 2 == 1);
    }
}

/// Threads fill, launder, allocate and free at once while another trims and beats the heart.
/// Each thread stamps its blocks with its own byte and checks the stamp before freeing, so a
/// slot handed to two threads at once, or purged under one, shows as a changed byte; a shared
/// set catches the same slot held twice directly.
#[test]
#[cfg_attr(miri, ignore)]
fn threads_never_share_a_slot() {
    let state = state::<Standard>();
    state.get_or_reserve().unwrap();
    let state = &*state;
    let held: Mutex<BTreeSet<usize>> = Mutex::new(BTreeSet::new());
    let classes = [small(16), small(48), small(1024)];
    let big = large(40 << 10);
    thread::scope(|scope| {
        for t in 0..4u8 {
            let held = &held;
            scope.spawn(move || {
                let back = state.get().unwrap();
                let mut rng = Rng(0x1234_5678 + t as u64);
                let stamp = t + 1;
                for _ in 0..300 {
                    let class = classes[rng.below(classes.len())];
                    let mut got = fill(&back, class, 1 + rng.below(60));
                    for &p in &got {
                        assert!(held.lock().unwrap().insert(p.addr()), "a slot held twice");
                        paint(p, class.size(), stamp);
                    }
                    let block = back.alloc(big, big.size());
                    if let Some(b) = block {
                        assert!(held.lock().unwrap().insert(b.ptr.addr().get()));
                        paint(b.ptr.as_ptr(), big.size(), stamp);
                    }
                    thread::yield_now();
                    for &p in &got {
                        assert!(
                            painted(p, class.size(), stamp),
                            "another thread wrote my slot"
                        );
                        held.lock().unwrap().remove(&p.addr());
                    }
                    back.launder(class, &mut got);
                    if let Some(b) = block {
                        assert!(painted(b.ptr.as_ptr(), big.size(), stamp));
                        held.lock().unwrap().remove(&b.ptr.addr().get());
                        back.free(b.ptr, big);
                    }
                }
            });
        }
        scope.spawn(|| {
            let back = state.get().unwrap();
            for i in 0..300 {
                if i % 10 == 0 {
                    back.trim();
                }
                back.heartbeat();
                thread::yield_now();
            }
        });
    });
}

/// Evidence for the adversarial review of the memory-return machinery. Each test fails on the
/// code as reviewed and is ignored so the gate passes; run one with
/// `cargo test -p asmalloc --lib back::tests::review -- --ignored <name>`.
/// A large object that shrinks in place gives back the whole pages past its new usable size:
/// they are not resident afterwards, the part kept is intact, no retained count moves, and a
/// grow back in place finds zeroes there. A shrink inside a page makes no call.
#[test]
#[cfg_attr(miri, ignore)]
fn a_large_slot_that_shrinks_in_place_returns_its_tail() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let page = Native::GEOMETRY.page_size();
    let class = large(64 << 10);
    let block = back.alloc(class, class.size()).unwrap();
    let ptr = block.ptr;
    paint(ptr.as_ptr(), class.size(), 0x6b);
    let purges = os.calls(Call::Purge);
    // To five pages and a byte: six pages stay usable.
    back.resize_in_place(ptr, 5 * page + 1);
    assert_eq!(back.usable_size(ptr.as_ptr()), Some(6 * page));
    assert_eq!(os.calls(Call::Purge), purges + 1);
    let tail = ptr.map_addr(|a| a.saturating_add(6 * page));
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe {
        assert!(os.is_purged(tail, class.size() - 6 * page));
        assert!(!os.is_purged(ptr, 6 * page));
    }
    assert!(painted(ptr.as_ptr(), 6 * page, 0x6b));
    assert_eq!(back.retained(), 0);
    // Within the last page: recorded, and nothing to return.
    back.resize_in_place(ptr, 5 * page + 100);
    back.resize_in_place(ptr, 5 * page + 7);
    assert_eq!(os.calls(Call::Purge), purges + 1);
    // Grown back in place: the old bytes are gone, not stale.
    back.resize_in_place(ptr, class.size());
    assert_eq!(back.usable_size(ptr.as_ptr()), Some(class.size()));
    assert_eq!(os.calls(Call::Purge), purges + 1);
    assert!(painted(ptr.as_ptr(), 6 * page, 0x6b));
    assert!(painted(tail.as_ptr(), class.size() - 6 * page, 0));
    // Shrunk again after being written, then freed and reused: the counts stay in step.
    paint(ptr.as_ptr(), class.size(), 0x11);
    back.resize_in_place(ptr, 2 * page);
    assert_eq!(os.calls(Call::Purge), purges + 2);
    back.free(ptr, class);
    assert_eq!(back.retained_large(), class.size());
    let again = back.alloc(class, class.size()).unwrap();
    assert_eq!(again.ptr, ptr);
    assert!(!again.zeroed);
    assert_eq!(back.retained(), 0);
    back.free(again.ptr, class);
}

/// The same for a run of regions.
#[test]
#[cfg_attr(miri, ignore)]
fn a_run_that_shrinks_in_place_returns_its_tail() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let page = Standard::GEOMETRY.page_size();
    let region = back.space().plan().region_size();
    let ptr = back.alloc_run(2 * region, 8).unwrap();
    paint(ptr.as_ptr(), 2 * region, 0x2c);
    let purges = os.calls(Call::Purge);
    back.resize_in_place(ptr, region / 2 + 1);
    let kept = region / 2 + page;
    assert_eq!(back.usable_size(ptr.as_ptr()), Some(kept));
    assert_eq!(os.calls(Call::Purge), purges + 1);
    let tail = ptr.map_addr(|a| a.saturating_add(kept));
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe {
        assert!(os.is_purged(tail, 2 * region - kept));
        assert!(!os.is_purged(ptr, kept));
    }
    assert!(painted(ptr.as_ptr(), kept, 0x2c));
    back.resize_in_place(ptr, 2 * region);
    assert!(painted(tail.as_ptr(), 2 * region - kept, 0));
    back.free_run(ptr);
}

/// Bytes that differ along a block, so a piece copied to the wrong place shows.
fn pattern(ptr: *mut u8, len: usize) {
    for i in 0..len {
        // SAFETY: the tests only write blocks the back end handed out.
        unsafe { ptr.add(i).write((i / 7 + i / 4096) as u8 | 1) };
    }
}

fn patterned(ptr: *const u8, len: usize) -> bool {
    // SAFETY: as in `pattern`.
    (0..len).all(|i| unsafe { ptr.add(i).read() } == (i / 7 + i / 4096) as u8 | 1)
}

/// A large block that moves is copied a piece at a time with the old block's pages returned
/// behind the copy: the move never has more resident than it started with plus one piece,
/// the contents arrive intact, and the old slot ends as purged memory, zero, uncounted and
/// not purged again by its free.
#[test]
#[cfg_attr(miri, ignore)]
fn a_moving_large_block_releases_its_old_pages_behind_the_copy() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let piece = move_piece(Native::GEOMETRY);
    let (old_class, new_class) = (large(512 << 10), large(1 << 20));
    let len = old_class.size() - 100;
    assert!(len > 4 * piece);
    let old = back.alloc(old_class, len).unwrap().ptr;
    pattern(old.as_ptr(), len);
    let new = back.alloc(new_class, new_class.size()).unwrap().ptr;
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    let resident = || unsafe { os.resident_bytes() };
    let before = resident();
    let purges = os.calls(Call::Purge) as usize;
    let (mut peak, mut pieces) = (0, 0);
    // SAFETY: `old` is a live block of `len` bytes that the test gives up, `new` a live block
    // of more, and nothing else uses either.
    unsafe {
        back.move_large_with(old, new, len, || {
            peak = peak.max(resident());
            pieces += 1;
        });
    }
    assert_eq!(pieces, len.div_ceil(piece));
    assert!(
        peak <= before + piece + Native::GEOMETRY.page_size(),
        "the move had {} bytes more resident than it began with; a piece is {piece}",
        peak - before
    );
    assert!(patterned(new.as_ptr(), len));
    // One call for each whole piece, one for the rest of the slot, and none at the free.
    assert_eq!(os.calls(Call::Purge) as usize, purges + len / piece + 1);
    assert_eq!(back.retained(), 0);
    // SAFETY: as above.
    unsafe {
        assert!(os.is_purged(old, old_class.size()));
    }
    let again = back.alloc(old_class, old_class.size()).unwrap();
    assert_eq!(again.ptr, old);
    assert!(again.zeroed && painted(old.as_ptr(), old_class.size(), 0));
    // A block of one piece or less is copied whole and freed the ordinary way.
    let small_class = large(piece);
    let one = back.alloc(small_class, piece).unwrap().ptr;
    pattern(one.as_ptr(), piece);
    let purges = os.calls(Call::Purge);
    // SAFETY: as above.
    unsafe { back.move_large(one, new, piece) };
    assert!(patterned(new.as_ptr(), piece));
    assert_eq!(os.calls(Call::Purge), purges);
    assert_eq!(back.retained_large(), small_class.size());
}

/// The same for a run of regions moving to a larger run.
#[test]
#[cfg_attr(miri, ignore)]
fn a_moving_run_releases_its_old_pages_behind_the_copy() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let piece = move_piece(Standard::GEOMETRY);
    let region = back.space().plan().region_size();
    let len = 2 * region - 5;
    let old = back.alloc_run(len, 8).unwrap();
    pattern(old.as_ptr(), len);
    let new = back.alloc_run(3 * region, 8).unwrap();
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    let resident = || unsafe { os.resident_bytes() };
    let before = resident();
    let mut peak = 0;
    // SAFETY: `old` is a live run of `len` bytes that the test gives up, `new` a live run of
    // more, and nothing else uses either.
    unsafe { back.move_large_with(old, new, len, || peak = peak.max(resident())) };
    assert!(peak <= before + piece + Standard::GEOMETRY.page_size());
    assert!(patterned(new.as_ptr(), len));
    assert_eq!(back.lookup(old.as_ptr()), None, "the old run is free");
    back.free_run(new);
}

mod review {
    use super::super::layout::Grow;
    use super::super::tree::Host;
    use super::super::word::Word;
    use super::*;
    use crate::classes::MagazineSize;
    use crate::space::Geometry;

    /// 4 KiB pages, 256 KiB huge pages, 4 MiB regions: a huge page of 8-byte slots is 512 leaf
    /// words, more than a heartbeat's purge budget, as it is in the design's geometry for every
    /// class up to 128 bytes. The other shapes' huge pages are too small to show that.
    enum Wide {}

    impl Shape for Wide {
        const GEOMETRY: Geometry = Geometry::new(12, 18, 22, 22, 16, 1, MagazineSize::MAX);
    }

    fn carved<O: Os>(back: &Back<'_, O>, class: Class, at: *mut u8) -> u32 {
        let host = back.host(class).unwrap();
        let region = back.space().locate(at).unwrap().region;
        Grow::unpack(host.meta(region, host.layout().grow_word()).load()).carved
    }

    /// Finding decay-walk-wedge. `purge_region` calls `top_up` whenever a region's cursor is
    /// zero and charges the words it read against the step's budget before the first unit. If
    /// the watermark's huge page has a live slot more than a budget's worth of leaf words above
    /// the page's start, `top_up` alone spends the budget, the loop breaks before any unit, the
    /// cursor stays zero, and the next heartbeat does the same. The walk never leaves the
    /// region: the pass never ends, and nothing in that region or any region below it is
    /// returned by the clock.
    #[test]
    fn a_live_slot_high_in_the_watermark_page_wedges_the_decay_walk() {
        let state = state::<Wide>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let huge = Wide::GEOMETRY.huge_page_size();
        // Region 0: a class that is wholly free, which the clock should return.
        let low = small(1024);
        let mut victims = fill(&back, low, 512);
        for &p in &victims {
            paint(p, 1024, 7);
        }
        // Region 1: 8-byte slots, into the huge-page part and three quarters of the way up a
        // huge page. Everything is freed but the last slot carved.
        let class = small(8);
        let mut slots = Vec::new();
        while slots.len() < (6 * huge + 3 * huge / 4) / 8 {
            slots.extend(fill(&back, class, 64));
        }
        for &p in &slots {
            paint(p, 8, 9);
        }
        // (With the live slot under a budget's worth of words from the page's start, say
        // `slots.remove(slots.len() - 20000)`, the pass ends and this test passes.)
        let live = slots.pop().unwrap();
        let region = back.space().locate(live).unwrap().region;
        let end = back
            .space()
            .region(region)
            .map_addr(|a| a.saturating_add(Wide::GEOMETRY.largest_plan().region_size() - 1));
        assert_eq!(os.huge_pages(end), Some(HugePages::Prefer));
        for chunk in slots.chunks_mut(64) {
            back.launder(class, chunk);
        }
        back.launder(low, &mut victims);
        assert!(back.retained_small() < retained_cap(Wide::GEOMETRY));
        for _ in 0..10 {
            os.advance_clock(DECAY_TICKS);
            beat(&back);
        }
        let laps = state.laps.load(core::sync::atomic::Ordering::Relaxed);
        let at = state.purge_at.load(core::sync::atomic::Ordering::Relaxed);
        assert!(
            !pass_under_way(&state) && os.calls(Call::Purge) > 0,
            "after ten intervals of {BEATS} heartbeats the pass is still under way: \
             {laps} laps, walk at region {at}, {} purges, {} bytes retained",
            os.calls(Call::Purge),
            back.retained()
        );
    }

    /// Finding count-drifts-up. `fill` subtracts what its claims found resident before it adds
    /// what they revived, and the subtraction stops at zero. A fill from a purged page with the
    /// count near zero loses the subtraction and keeps the addition: bytes are counted as
    /// retained though every slot is in the program's hands. The same loss happens across
    /// threads when a claim's `unretain` runs between a free's `release` and its `retain`
    /// (`class.rs`, `release_all`): there the lost amount is a whole large slot.
    #[test]
    fn a_fill_from_purged_memory_leaves_bytes_counted_that_are_not_free() {
        let state = state::<Standard>();
        let back = state.get_or_reserve().unwrap();
        let class = small(16);
        let page = Standard::GEOMETRY.page_size();
        let mut slots = fill(&back, class, page / 16);
        back.launder(class, &mut slots);
        assert_eq!(back.trim(), page);
        assert_eq!(back.retained(), 0);
        let again = fill(&back, class, page / 16);
        assert_eq!(again, slots);
        assert_eq!(
            back.retained(),
            0,
            "the program holds every slot, yet bytes are counted as free and resident"
        );
    }

    /// Finding count-drifts-up, across threads, for the large-class count. A race, so it is
    /// evidence when it fails and none when it passes: two threads allocate and free slots of
    /// one large class that is bigger than the large cap. Each free purges its own slot, so
    /// with nothing allocated the count should be zero. When one thread's claim lowers the
    /// count between the other's `release` and `retain`, the subtraction is lost at zero and
    /// the addition stays; the freeing thread then finds its slot taken and cannot purge it.
    /// The count is left over the cap with nothing free, and from then on every large free
    /// purges its slot.
    #[test]
    fn racing_large_frees_leave_the_large_count_over_its_cap_with_nothing_free() {
        let state = state::<Native>();
        state.get_or_reserve().unwrap();
        let state = &*state;
        let class = large(512 << 10);
        assert!(class.size() > large_retained_cap(Native::GEOMETRY));
        thread::scope(|scope| {
            // More threads than CPUs, so that some are preempted between the two steps.
            for _ in 0..64 {
                scope.spawn(move || {
                    let back = state.get().unwrap();
                    for _ in 0..20_000 {
                        let a = back.alloc(class, class.size()).unwrap();
                        let b = back.alloc(class, class.size()).unwrap();
                        back.free(a.ptr, class);
                        back.free(b.ptr, class);
                    }
                });
            }
        });
        let back = state.get().unwrap();
        back.trim();
        assert_eq!(
            back.retained_large(),
            0,
            "every slot is free and purged, yet the count says this much is resident"
        );
    }

    /// Finding cap-has-no-hysteresis. `cap_fires` looks only at the two ends of one launder's
    /// own addition, so a count that hovers at the cap (or at any re-arm step) fires on every
    /// launder that crosses upward: here each one walks the whole fragmented heap for nothing.
    #[test]
    fn a_count_hovering_at_the_cap_walks_the_heap_on_every_launder() {
        let state = state::<Standard>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let cap = retained_cap(Standard::GEOMETRY);
        let class = small(1024);
        let per_page = Standard::GEOMETRY.page_size() / 1024;
        let slots = fill(&back, class, 2 * cap / 1024);
        // One slot of every page stays live, so nothing can be purged.
        let (_live, mut storm): (Vec<*mut u8>, Vec<*mut u8>) = slots
            .iter()
            .partition(|p| (p.addr() / 1024).is_multiple_of(per_page));
        let mut extra = storm.split_off(cap / 1024 - 32);
        extra.truncate(64);
        for chunk in storm.chunks_mut(64) {
            back.launder(class, chunk);
        }
        assert_eq!(back.retained_small(), cap - 32 * 1024);
        let laps = || state.laps.load(core::sync::atomic::Ordering::Relaxed);
        let before = laps();
        // The program frees 64 slots and takes 64 back, over and over, so the count moves
        // between 32 KiB under the cap and 32 KiB over it.
        for _ in 0..50 {
            back.launder(class, &mut extra);
            assert!(back.retained_small() > cap);
            extra = fill(&back, class, 64);
            assert!(back.retained_small() <= cap);
        }
        assert_eq!(os.calls(Call::Purge), 0, "nothing was purgeable");
        assert!(
            laps() - before <= 2,
            "50 launders across the cap walked the heap {} times",
            laps() - before
        );
    }

    /// Finding clock-back-stops-decay. `decay_look` reads a clock behind the last pass's end as
    /// "not yet", with no limit: after the counter goes back (resume from suspend resets the
    /// timestamp counter on most machines; a restored checkpoint) nothing decays until the
    /// counter has caught up with where it was, however long that is.
    #[test]
    fn a_clock_that_went_back_stops_decay_until_it_catches_up() {
        let state = state::<Standard>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let class = small(1024);
        let total = retained_cap(Standard::GEOMETRY) / 2;
        let mut slots = fill(&back, class, total / 1024);
        for &p in &slots {
            paint(p, 1024, 5);
        }
        back.launder(class, &mut slots);
        os.advance_clock(100 * DECAY_TICKS);
        beat(&back);
        assert!(!pass_under_way(&state));
        let left = back.retained();
        assert!(left > 0 && left < total);
        // The counter goes back by fifty intervals.
        os.advance_clock(0u64.wrapping_sub(50 * DECAY_TICKS));
        for _ in 0..20 {
            os.advance_clock(DECAY_TICKS);
            beat(&back);
        }
        assert!(
            back.retained() < left,
            "twenty intervals after the clock went back, nothing has decayed"
        );
    }

    /// Finding decay-overshoots-goal. A pass's goal is checked only between steps, and a step
    /// is bounded in words of bitmap, not bytes. A word covers 64 slots, so one step of 256
    /// words in a region of large slots covers far more than a quarter of what is retained:
    /// in the design's geometry 16 MiB of 1 KiB slots, 512 MiB of 32 KiB ones. The first pass
    /// returns everything, where the design says up to a quarter.
    #[test]
    fn a_decay_pass_over_big_slots_returns_everything_at_once() {
        let state = state::<Wide>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let class = small(32 << 10);
        let total = 3 << 20;
        let mut slots = fill(&back, class, total / class.size());
        for &p in &slots {
            paint(p, class.size(), 5);
        }
        back.launder(class, &mut slots);
        assert_eq!(back.retained(), total);
        assert!(total < retained_cap(Wide::GEOMETRY));
        os.advance_clock(DECAY_TICKS);
        beat(&back);
        assert!(!pass_under_way(&state));
        let left = back.retained();
        assert!(
            left >= total / 2,
            "one pass returned {} of {total} bytes",
            total - left
        );
    }

    /// Finding decay-start-is-four-writes. A stepper that sees a pass under way before the
    /// starter has published its goal must not take the pass word left by the last pass (its
    /// goal met, so zero) for this one's and end the new pass. The window is set up by hand:
    /// a decay word that says a pass began, and the pass word of an earlier pass.
    #[test]
    fn a_stepper_does_not_end_a_pass_whose_goal_is_not_published() {
        use super::super::purge::Pass;
        use core::sync::atomic::Ordering::Relaxed;
        let state = state::<Standard>();
        let back = state.get_or_reserve().unwrap();
        let class = small(1024);
        let mut slots = fill(&back, class, 256);
        for &p in &slots {
            paint(p, 1024, 5);
        }
        back.launder(class, &mut slots);
        // The pass began just now: one that stays unpublished for an interval is another
        // matter (`a_pass_whose_starter_vanished_before_publishing_ends_some_day`).
        state.os().advance_clock(7 * DECAY_TICKS);
        let started = (7 * DECAY_TICKS) | 1;
        let stale = Pass {
            tag: Pass::tag_of(started) ^ 1,
            lap: 0,
            pages: 0,
        };
        state.pass.store(stale.pack(), Relaxed);
        state.decay.store(started, Relaxed);
        beat(&back);
        assert_eq!(
            state.decay.load(Relaxed),
            started,
            "a stepper ended a pass it had no goal for"
        );
        assert_eq!(state.os().calls(Call::Purge), 0);
        // The starter publishes; the pass now runs and ends.
        let goal = Pass {
            tag: Pass::tag_of(started),
            lap: state.laps.load(Relaxed) as u16,
            pages: 16,
        };
        state.pass.store(goal.pack(), Relaxed);
        beat(&back);
        assert!(!pass_under_way(&state));
        assert_eq!(state.os().calls(Call::Purge), 16);
    }

    /// Finding abandoned-pass-start-stops-decay (opened by the fix for the four-write start).
    /// The starter's compare-and-swap of the decay word and its store of the pass word are
    /// two writes. A thread that vanishes between them (a fork abandons it there) leaves a
    /// decay word that says a pass is under way and no pass word with its tag. Every stepper
    /// then finds a tag that is not its own and returns, and nothing but a stepper ends a
    /// pass or starts one: the child never decays again. The state is set up by hand, as in
    /// `a_stepper_does_not_end_a_pass_whose_goal_is_not_published`, which asserts the first
    /// half of this as intended; here the starter never comes back.
    #[test]
    fn a_pass_whose_starter_vanished_before_publishing_ends_some_day() {
        use super::super::purge::Pass;
        use core::sync::atomic::Ordering::Relaxed;
        let state = state::<Standard>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let class = small(1024);
        let mut slots = fill(&back, class, 256);
        for &p in &slots {
            paint(p, 1024, 5);
        }
        back.launder(class, &mut slots);
        os.advance_clock(7 * DECAY_TICKS);
        let started = (7 * DECAY_TICKS) | 1;
        assert_ne!(
            Pass::tag_of(started),
            0,
            "the all-zero pass word is not this pass's"
        );
        state.decay.store(started, Relaxed);
        for _ in 0..50 {
            os.advance_clock(DECAY_TICKS);
            beat(&back);
        }
        assert!(
            !pass_under_way(&state) && os.calls(Call::Purge) > 0,
            "fifty intervals on, the abandoned pass is still under way and nothing has decayed: \
             {} bytes retained",
            back.retained()
        );
    }

    /// Finding cap-purge-stops-at-the-lap-count (opened by the fix for the cap's hysteresis).
    /// The cap's purge now stops when the lap count changes, and it starts wherever the shared
    /// walk was left. If that is near the bottom, it looks at the few regions below, the lap
    /// ends, and it stops without having seen the regions that hold the freed memory. The
    /// mark is then set to the count, so the cap does not act again until a quarter-cap more
    /// is freed. Here the walk was left at region 0 (set by hand; a decay pass that met its
    /// goal there, or a cap purge that got under the cap there, leaves it so), and the
    /// program frees more than the cap, all of it purgeable, in the regions above.
    #[test]
    fn the_cap_purges_wherever_the_walk_was_left() {
        use core::sync::atomic::Ordering::Relaxed;
        let state = state::<Standard>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let cap = retained_cap(Standard::GEOMETRY);
        // Region 0 goes to a class that stays in use.
        let _live = fill(&back, small(16), 8);
        let class = small(1024);
        let mut slots = fill(&back, class, cap / 1024 + 128);
        for &p in &slots {
            paint(p, 1024, 5);
        }
        state.purge_at.store(1, Relaxed);
        for chunk in slots.chunks_mut(64) {
            back.launder(class, chunk);
        }
        assert!(
            os.calls(Call::Purge) > 0 && back.retained_small() <= cap,
            "{} bytes are free and wholly purgeable, the cap is {cap}, and the cap's purge \
             returned nothing",
            back.retained_small()
        );
    }

    /// Finding top-up-creeps. For a class whose size does not divide a huge page, `top_up`
    /// carves through the slot that straddles the page's end, which leaves the watermark just
    /// inside the next huge page with one free slot below it. The next pass over the region
    /// takes that for a watermark page worth purging and carves all of it too, though it was
    /// never touched, and so on until a huge-page boundary that the size divides.
    #[test]
    fn topping_up_a_straddled_huge_page_carves_untouched_ones_on_later_passes() {
        let state = state::<Native>();
        let back = state.get_or_reserve().unwrap();
        let class = small(48);
        let huge = Native::GEOMETRY.huge_page_size();
        let mut slots = Vec::new();
        while slots.len() < (6 * huge + huge / 2) / 48 {
            slots.extend(fill(&back, class, 64));
        }
        let top = slots[slots.len() - 1];
        for chunk in slots.chunks_mut(64) {
            back.launder(class, chunk);
        }
        back.trim();
        let first = carved(&back, class.class(), top);
        back.trim();
        back.trim();
        let third = carved(&back, class.class(), top);
        assert_eq!(
            first,
            third,
            "an idle region's watermark moved {} bytes over two more passes",
            (third - first) as usize * 48
        );
    }
}
