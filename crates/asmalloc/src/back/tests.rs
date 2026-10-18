//! The back end over the simulated OS: unit tests, fault injection, a shadow-model property test
//! and a threaded stress test.

use super::policy::{
    DECAY_TICKS, STAGGER_ROOM, STAGGER_STEP, growth_goal, growth_step, huge_after, kept_grown,
    move_piece, retained_cap,
};
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
    let got = back.fill(class, &mut out[..]);
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

/// With 8-byte slots the leaf bitmap of a region fills pages of its own, after the pages of the
/// summary and clean bitmap, so it is committed only as carves need it. Freeing slots
/// writes it; the simulator's memory is inaccessible until committed, so a write the carve did
/// not commit for kills the test.
#[test]
fn the_leaf_bitmap_is_committed_before_it_is_written() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(8);
    let slots = fill(&back, class, 3000);
    assert_eq!(slots.len(), 3000);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &slots) };
    assert_eq!(fill(&back, class, 3000).len(), 3000);
    back.trim();
}

#[test]
fn launder_then_fill_reuses_the_lowest_free_slots_first() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(64);
    let slots = fill(&back, class, 200);
    let freed: Vec<*mut u8> = slots[10..60].iter().rev().copied().collect();
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &freed) };
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
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &[slots[1]]) };
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &[slots[1]]) };
}

#[test]
#[should_panic(expected = "freed twice in one batch")]
fn a_slot_twice_in_one_batch_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(16);
    let slots = fill(&back, class, 4);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &[slots[1], slots[2], slots[1]]) };
}

/// A slot that comes back after its word was ORed, the batch having gone on in order, is
/// caught by the word's OR.
#[test]
#[should_panic(expected = "double free")]
fn a_slot_twice_after_its_word_closed_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(16);
    let slots = fill(&back, class, 64 * 8);
    let batch: Vec<*mut u8> = (0..6).map(|w| slots[w * 64]).chain([slots[0]]).collect();
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &batch) };
}

/// The same when the word was met again before the slot comes back: in the map or by its OR,
/// the second free is caught.
#[test]
#[should_panic(expected = "double free")]
fn a_slot_twice_across_a_reopened_word_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(16);
    let slots = fill(&back, class, 64 * 8);
    let mut batch: Vec<*mut u8> = (0..6).map(|w| slots[w * 64]).collect();
    batch.extend([slots[1], slots[0]]);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &batch) };
}

/// A scattered batch remembers every word it has met, however many: a different slot of a
/// word met long before is no double free; the same slot again is.
#[test]
#[should_panic(expected = "freed twice in one batch")]
fn a_slot_twice_after_many_closed_words_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(16);
    let slots = fill(&back, class, 64 * 300);
    // Out of order, so that the batch keeps its words in the map.
    let words: Vec<*mut u8> = (0..250).map(|w| slots[w * 97 % 250 * 64]).collect();
    let mut fine = words.clone();
    fine.push(slots[1]);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &fine) };
    assert_eq!(fill(&back, class, 251).len(), 251);
    let mut twice = words.clone();
    // A word met after the order broke, so in the map.
    twice.push(words[100]);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &twice) };
}

/// A batch longer than its map of words holds flushes the map when it fills; the same slot
/// on both sides of a flush is caught by its word's OR.
#[test]
#[should_panic(expected = "double free: an object was freed while already free")]
fn a_slot_twice_across_a_full_map_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(16);
    let slots = fill(&back, class, 64 * 700);
    let mut twice: Vec<*mut u8> = (0..650).map(|w| slots[w * 97 % 650 * 64]).collect();
    // Word 194 is the first to go to the map, outside the window around word 0, and is ORed
    // when the map first fills.
    twice.push(slots[194 * 64]);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &twice) };
}

/// Scattered frees, one slot to a word across several summary words, and frees that close and
/// reopen words, all reach the bitmaps: a fill afterwards gets every slot back, lowest first.
#[test]
fn scattered_launders_free_every_slot_and_fills_take_the_lowest() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(8);
    // 8-byte slots in the standard geometry: two summary levels, 64 leaf words to a summary
    // word, so this spans four of them.
    let n = 64 * 64 * 4;
    let mut slots = fill(&back, class, n);
    slots.sort();
    let order: Vec<*mut u8> = (0..n).map(|i| slots[i * 7919 % n]).collect();
    for chunk in order.chunks(64) {
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, chunk) };
    }
    let again = fill(&back, class, n);
    assert_eq!(again, slots, "every slot back, in address order");
    // Now a scattered subset: a fill takes exactly those, in order, before carving.
    let some: Vec<*mut u8> = slots.iter().step_by(37).copied().collect();
    let shuffled: Vec<*mut u8> = (0..some.len()).map(|i| some[i * 31 % some.len()]).collect();
    assert_ne!(some.len() % 31, 0, "a permutation");
    for chunk in shuffled.chunks(64) {
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, chunk) };
    }
    assert_eq!(fill(&back, class, some.len()), some);
}

#[test]
#[should_panic(expected = "not the start of a slot")]
fn freeing_an_interior_pointer_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(32);
    let slots = fill(&back, class, 4);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &[slots[1].wrapping_add(8)]) };
}

#[test]
#[should_panic(expected = "wrong class")]
fn freeing_with_the_wrong_class_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let slots = fill(&back, small(32), 4);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(small(64), &[slots[0]]) };
}

#[test]
#[should_panic(expected = "never handed out")]
fn freeing_an_uncarved_slot_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(32);
    let slots = fill(&back, class, 4);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &[slots[3].wrapping_add(32)]) };
}

#[test]
#[should_panic(expected = "outside the heap")]
fn freeing_a_foreign_pointer_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let mut local = 0u8;
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(small(8), &[&raw mut local]) };
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
fn the_huge_page_part_starts_at_the_threshold() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = small(64);
    let huge = Native::GEOMETRY.huge_page_size();
    let threshold = huge_after(back.space().plan().region_size() / huge) * huge;
    // Past the threshold by a huge page, a magazine at a time. Small-page commits run ahead of
    // the carve, so a switch noticed only after the carve passed the threshold would start the
    // huge-page part a huge page above it.
    let mut slots = Vec::new();
    while slots.len() < (threshold + huge) / 64 {
        slots.extend(fill(&back, class, 254));
    }
    let base = back
        .space()
        .region(back.space().locate(slots[0]).unwrap().region);
    let advice = |offset: usize| {
        state
            .os()
            .huge_pages(base.map_addr(|a| a.saturating_add(offset)))
    };
    assert_eq!(advice(threshold - 1), Some(HugePages::Avoid));
    assert_eq!(advice(threshold), Some(HugePages::Prefer));
}

#[test]
#[cfg_attr(miri, ignore)]
fn large_slots_say_when_they_are_zero() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = large(64 << 10);
    let Block { ptr, zeroed, .. } = back.alloc(class, class.size()).unwrap();
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
/// slots): freed past the cap they all stay resident; another class's growth then purges
/// them, top down, until the count is under the cap; the pages leave the resident set; and
/// the purged slots come back zeroed, with their zero bits set and cleared exactly once.
#[test]
#[cfg_attr(miri, ignore)]
fn large_slots_past_the_cap_are_purged_by_growth_and_come_back_zeroed() {
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
    let purges = os.calls(Call::Purge);
    for b in &blocks {
        back.free(b.ptr, class);
    }
    assert_eq!(os.calls(Call::Purge), purges, "a free purged");
    assert_eq!(back.retained(), n * class.size());
    // Another class grows by as much as is over the cap, and pays for it.
    let before = resident();
    let other = large(128 << 10);
    let grown: Vec<Block> = (0..(n * class.size() - cap) / other.size())
        .map(|_| back.alloc(other, other.size()).unwrap())
        .collect();
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
    for b in grown {
        back.free(b.ptr, other);
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
    let some = slots.split_off(all - 3);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &some) };
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
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &slots) };
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

/// A grower purges nothing for less than a step, nothing at or under the cap, and otherwise
/// as much as it grew or what is over the cap, whichever is less.
#[test]
fn growth_pays_for_what_it_grew_up_to_what_is_over_the_cap() {
    let (cap, step) = (64 << 20, 2 << 20);
    assert_eq!(growth_goal(step - 1, 2 * cap, cap, step), 0);
    assert_eq!(growth_goal(0, 2 * cap, cap, step), 0);
    assert_eq!(growth_goal(step, cap, cap, step), 0);
    assert_eq!(growth_goal(step, 0, cap, step), 0);
    assert_eq!(growth_goal(step, 2 * cap, cap, step), step);
    assert_eq!(growth_goal(step, cap + 4096, cap, step), 4096);
    assert_eq!(growth_goal(1 << 30, 2 * cap, cap, step), cap);
    assert_eq!(growth_goal(usize::MAX, usize::MAX, 0, step), usize::MAX);
}

/// Freeing far past the cap purges nothing, and neither does reusing what was freed: reuse is
/// not growth. Another class's growth then purges about as much as it grows, from the top,
/// until the count is at the cap, and no more.
#[test]
#[cfg_attr(miri, ignore)]
fn frees_and_reuse_never_purge_and_growth_pays_for_itself() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let cap = retained_cap(Standard::GEOMETRY);
    let step = growth_step(Standard::GEOMETRY);
    let class = small(1024);
    let n = 3 * cap / 1024;
    let slots = fill(&back, class, n);
    assert_eq!(slots.len(), n);
    let purges = os.calls(Call::Purge);
    for _ in 0..4 {
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, &slots) };
        assert_eq!(os.calls(Call::Purge), purges, "a free purged");
        assert_eq!(back.retained(), n * 1024);
        assert_eq!(fill(&back, class, n), slots, "reuse took other slots");
        assert_eq!(os.calls(Call::Purge), purges, "reuse purged");
    }
    // SAFETY: as above.
    unsafe { back.launder(class, &slots) };
    // Another class grows by one step at a time; each step purges about one step.
    let other = small(512);
    let mut grown = Vec::new();
    let mut last = back.retained();
    while last > cap {
        grown.extend(fill(&back, other, step / 512));
        let now = back.retained();
        assert!(
            last - now <= 2 * step,
            "one step of growth purged {} bytes",
            last - now
        );
        last = now;
    }
    assert!(grown.len() * 512 <= n * 1024 - cap + 2 * step);
    // At the cap, growth purges nothing more.
    let purges = os.calls(Call::Purge);
    grown.extend(fill(&back, other, 4 * step / 512));
    assert_eq!(os.calls(Call::Purge), purges);
    // The purged pages are the top ones: the lowest slots are still resident.
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe {
        assert!(!os.is_purged(NonNull::new(slots[0]).unwrap(), 1024));
        assert!(os.is_purged(NonNull::new(slots[n - 1]).unwrap(), 1024));
    }
}

/// Taking purged pages back is reuse, not growth: with free memory over the cap, a class that
/// refills from pages it gave back purges nothing, so a purge never sets off another.
#[test]
#[cfg_attr(miri, ignore)]
fn reusing_purged_pages_over_the_cap_purges_nothing() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let cap = retained_cap(Standard::GEOMETRY);
    let other = small(512);
    let returned = fill(&back, other, cap / 512);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(other, &returned) };
    assert_eq!(back.trim(), cap);
    let class = small(1024);
    let slots = fill(&back, class, 3 * cap / 1024);
    // SAFETY: as above.
    unsafe { back.launder(class, &slots) };
    assert!(back.retained() > cap);
    let purges = os.calls(Call::Purge);
    assert_eq!(fill(&back, other, cap / 512), returned);
    assert_eq!(os.calls(Call::Purge), purges);
}

/// A heap whose free memory over the cap is all fragmentation pays one bounded walk for each
/// step of growth, and the walk purges nothing.
#[test]
#[cfg_attr(miri, ignore)]
fn growth_over_a_fragmented_heap_walks_once_a_step() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let cap = retained_cap(Standard::GEOMETRY);
    let step = growth_step(Standard::GEOMETRY);
    let class = small(1024);
    let per_page = Standard::GEOMETRY.page_size() / 1024;
    let slots = fill(&back, class, 3 * cap / 1024);
    let (_live, freed): (Vec<*mut u8>, Vec<*mut u8>) = slots
        .iter()
        .partition(|p| (p.addr() / 1024).is_multiple_of(per_page));
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &freed) };
    assert!(back.retained() > cap);
    // The walk starts at the top, so one circuit of it is one lap.
    let laps = || state.walk.laps.load(core::sync::atomic::Ordering::Relaxed);
    let other = small(512);
    for _ in 0..8 {
        let before = laps();
        let _ = fill(&back, other, step / 512 - 1);
        assert_eq!(laps(), before, "a fill under a step walked");
        let _ = fill(&back, other, 1);
        assert_eq!(laps(), before + 1, "a step of growth is one walk");
    }
    assert_eq!(os.calls(Call::Purge), 0);
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
/// still has a live neighbour while a purge runs, so it purges nothing. The count must keep
/// those bytes, so that when the neighbours are freed too the memory is still known and is
/// returned. The old code zeroed the count after a fruitless pass.
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
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, chunk) };
    }
    // A pass over the storm: every page has a live neighbour.
    os.advance_clock(DECAY_TICKS);
    beat(&back);
    assert_eq!(os.calls(Call::Purge), purges, "no page was wholly free");
    assert_eq!(back.retained(), freed, "the count forgot fragmented bytes");
    // The neighbours go too. Nothing is live now, and the memory comes back with the clock.
    for chunk in survivors.chunks_mut(64) {
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, chunk) };
    }
    assert_eq!(back.retained(), n * 1024);
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
    let slots = fill(&back, class, total / 1024);
    for &p in &slots {
        paint(p, 1024, 5);
    }
    let (lowest, highest) = (slots[0], slots[slots.len() - 1]);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &slots) };
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
    let slots = fill(&back, class, 64);
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &slots) };
    assert_eq!(back.retained(), 64 * 1024);
    let again = fill(&back, class, 16);
    assert_eq!(back.retained(), 48 * 1024);
    assert_eq!(back.trim(), 48 * 1024);
    assert_eq!(back.retained(), 0);
    // One slot of a purged page: the page's other slots are free and resident again.
    let one = fill(&back, class, 1);
    assert_eq!(back.retained(), page - 1024);
    let all: Vec<*mut u8> = again.into_iter().chain(one).collect();
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &all) };
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
    let (_live, freed): (Vec<*mut u8>, Vec<*mut u8>) = slots
        .iter()
        .partition(|p| (p.addr() / 1024).is_multiple_of(per_page));
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &freed) };
    let retained = back.retained();
    let laps = || state.walk.laps.load(core::sync::atomic::Ordering::Relaxed);
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

/// Freed large slots stay resident whatever their number, and a program that frees and
/// reallocates them never purges: the pipeline of large objects that faulted its slots back on
/// every round trip. A large allocation of another class then pays for its own growth.
#[test]
#[cfg_attr(miri, ignore)]
fn freed_large_slots_stay_until_growth_pays_for_them() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(64 << 10);
    let cap = retained_cap(Native::GEOMETRY);
    let n = 2 * cap / class.size();
    let blocks: Vec<Block> = (0..n)
        .map(|_| back.alloc(class, class.size()).unwrap())
        .collect();
    for b in &blocks {
        paint(b.ptr.as_ptr(), class.size(), 0x33);
    }
    let purges = os.calls(Call::Purge);
    for b in &blocks {
        back.free(b.ptr, class);
    }
    assert_eq!(back.retained(), n * class.size());
    for _ in 0..4 {
        let again: Vec<Block> = (0..n)
            .map(|_| back.alloc(class, class.size()).unwrap())
            .collect();
        assert!(again.iter().all(|b| !b.zeroed), "reuse takes a kept slot");
        for b in &again {
            back.free(b.ptr, class);
        }
    }
    assert_eq!(os.calls(Call::Purge), purges, "frees and reuse purged");
    // Another class's block: its growth purges as much as it asked for, and its free keeps it.
    let big = large(512 << 10);
    let block = back.alloc(big, big.size()).unwrap();
    assert!(os.calls(Call::Purge) > purges);
    let left = n * class.size() - back.retained();
    assert!(
        left >= big.size() && left < big.size() + class.size(),
        "growth of {} purged {left}",
        big.size()
    );
    paint(block.ptr.as_ptr(), big.size(), 1);
    let retained = back.retained();
    back.free(block.ptr, big);
    assert_eq!(back.retained(), retained + big.size());
    let again = back.alloc(big, big.size()).unwrap();
    assert!(!again.zeroed && painted(again.ptr.as_ptr(), big.size(), 1));
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
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, chunk) };
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
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, chunk) };
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
                        let batch = batch;
                        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
                        unsafe { back.launder(c.small().unwrap(), &batch) };
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

fn random_operations_match_the_shadow_model(seeds: u64) {
    let steps = if cfg!(miri) { 40 } else { 1500 };
    for seed in 0..seeds {
        shadow_run::<Standard>(0x9e37_79b9_7f4a_7c15 ^ seed, steps, false);
    }
}

#[test]
fn random_operations_match_the_shadow_model_quickly() {
    random_operations_match_the_shadow_model(if cfg!(miri) { 1 } else { 3 });
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn random_operations_match_the_shadow_model_fully() {
    random_operations_match_the_shadow_model(12);
}

fn random_operations_survive_os_failures(seeds: u64) {
    let steps = if cfg!(miri) { 40 } else { 1500 };
    for seed in 0..seeds {
        shadow_run::<Standard>(0x2545_f491_4f6c_dd1d ^ seed, steps, true);
    }
}

#[test]
fn random_operations_survive_os_failures_quickly() {
    random_operations_survive_os_failures(if cfg!(miri) { 1 } else { 3 });
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn random_operations_survive_os_failures_fully() {
    random_operations_survive_os_failures(12);
}

fn random_operations_with_several_large_slots_per_region(seeds: u64) {
    for seed in 0..seeds {
        shadow_run::<Native>(0x5851_f42d_4c95_7f2d ^ seed, 1500, seed % 2 == 1);
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn random_operations_with_several_large_slots_per_region_quickly() {
    random_operations_with_several_large_slots_per_region(2);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn random_operations_with_several_large_slots_per_region_fully() {
    random_operations_with_several_large_slots_per_region(4);
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
                    let got = fill(&back, class, 1 + rng.below(60));
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
                    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
                    unsafe { back.launder(class, &got) };
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
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(ptr, 5 * page + 1) };
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
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(ptr, 5 * page + 100) };
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(ptr, 5 * page + 7) };
    assert_eq!(os.calls(Call::Purge), purges + 1);
    // Grown back in place: the old bytes are gone, not stale. Nearly threefold at once is not
    // dense growth, so the slot gets no huge pages.
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(ptr, class.size()) };
    assert_eq!(back.usable_size(ptr.as_ptr()), Some(class.size()));
    assert_eq!(os.calls(Call::Purge), purges + 1);
    assert!(painted(ptr.as_ptr(), 6 * page, 0x6b));
    assert!(painted(tail.as_ptr(), class.size() - 6 * page, 0));
    assert_eq!(os.huge_pages(ptr), Some(HugePages::Avoid));
    // Shrunk again after being written, then freed and reused: the counts stay in step.
    paint(ptr.as_ptr(), class.size(), 0x11);
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(ptr, 2 * page) };
    assert_eq!(os.calls(Call::Purge), purges + 2);
    back.free(ptr, class);
    // Charged the two pages it may have written, not the slot.
    assert_eq!(back.retained(), 2 * page);
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
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(ptr, region / 2 + 1) };
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
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(ptr, 2 * region) };
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
    let ballast = spend_kept_budget(&back);
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
        back.move_large_with(old, new, len, true, || {
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
    assert_eq!(back.retained(), ballast);
    // SAFETY: as above.
    unsafe {
        assert!(os.is_purged(old, old_class.size()));
    }
    let again = back.alloc(old_class, old_class.size()).unwrap();
    assert_eq!(again.ptr, old);
    assert!(again.zeroed && painted(old.as_ptr(), old_class.size(), 0));
    // A block of one piece or less goes back the same way, in one purge, where a free would
    // keep it: an outgrown block left resident is the trail a program growing many buffers
    // leaves behind.
    let small_class = large(piece);
    let one = back.alloc(small_class, small_class.size()).unwrap().ptr;
    pattern(one.as_ptr(), small_class.size());
    let purges = os.calls(Call::Purge);
    // SAFETY: as above.
    unsafe { back.move_large(one, new, small_class.size(), false) };
    assert!(patterned(new.as_ptr(), small_class.size()));
    assert_eq!(os.calls(Call::Purge), purges + 1);
    assert_eq!(back.retained(), ballast);
    // SAFETY: as above.
    unsafe { assert!(os.is_purged(one, small_class.size())) };
    let again = back.alloc(small_class, small_class.size()).unwrap();
    assert_eq!(again.ptr, one);
    assert!(again.zeroed);
}

/// A block that grows by moving, densely and past a huge page, gets huge pages in its new
/// slot until it is freed; the free returns the whole slot and takes the advice back. A block
/// that grows by much more at once, or is allocated at its size, gets none.
#[test]
#[cfg_attr(miri, ignore)]
fn a_block_that_grows_gets_huge_pages_until_it_is_freed() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let ballast = spend_kept_budget(&back);
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let (old_class, new_class) = (large(96 << 10), large(512 << 10));
    let grow = |len: usize, size: usize| {
        let old = back.alloc(old_class, len).unwrap().ptr;
        pattern(old.as_ptr(), len);
        // As the heap moves a block: a new slot with room, its size recorded, then the move.
        let block = back.alloc(new_class, new_class.size()).unwrap();
        let new = block.ptr;
        back.resize(new, size);
        // SAFETY: both are live blocks of these sizes that nothing else uses.
        unsafe { back.move_large(old, new, len, block.zeroed) };
        assert!(patterned(new.as_ptr(), len));
        new
    };
    let dense = grow(80 << 10, 160 << 10);
    let at = |offset: usize| os.huge_pages(dense.map_addr(|a| a.saturating_add(offset)));
    // The whole huge pages the new size covers, and not the rest or the room past it.
    assert_eq!(at(0), Some(HugePages::Prefer));
    assert_eq!(at(2 * huge - 1), Some(HugePages::Prefer));
    assert_eq!(at(2 * huge), Some(HugePages::Avoid));
    // Growing on in place, densely, extends it.
    pattern(dense.as_ptr(), 160 << 10);
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(dense, 320 << 10) };
    assert_eq!(at(5 * huge - 1), Some(HugePages::Prefer));
    assert_eq!(at(5 * huge), Some(HugePages::Avoid));
    let purges = os.calls(Call::Purge);
    back.free(dense, new_class);
    assert_eq!(
        os.calls(Call::Purge),
        purges + 1,
        "the whole slot goes back at the free"
    );
    assert_eq!(os.huge_pages(dense), Some(HugePages::Avoid));
    assert_eq!(back.retained(), ballast);
    // Reused by a block that did not grow: no advice, and zero.
    let again = back.alloc(new_class, new_class.size()).unwrap();
    assert_eq!(again.ptr, dense);
    assert!(again.zeroed);
    back.free(again.ptr, new_class);
    // Growing more than twofold at once says nothing about density.
    let far = grow(80 << 10, 200 << 10);
    assert_eq!(os.huge_pages(far), Some(HugePages::Avoid));
    back.free(far, new_class);
    // Nor does a block whose pages are mostly empty: one page of 20 written.
    let old = back.alloc(old_class, 80 << 10).unwrap().ptr;
    paint(old.as_ptr(), 100, 0x33);
    let block = back.alloc(new_class, new_class.size()).unwrap();
    back.resize(block.ptr, 160 << 10);
    // SAFETY: both are live blocks that nothing else uses.
    unsafe { back.move_large(old, block.ptr, 80 << 10, block.zeroed) };
    assert_eq!(os.huge_pages(block.ptr), Some(HugePages::Avoid));
    assert!(painted(block.ptr.as_ptr(), 100, 0x33));
    back.free(block.ptr, new_class);
    // In place, the same rules: a dense block doubling is advised, a sparse one is not.
    for (written, advised) in [(huge / 2, true), (100, false)] {
        let block = back.alloc(new_class, huge / 2).unwrap().ptr;
        paint(block.as_ptr(), written, 0x44);
        // SAFETY: the test holds the block and uses none of it during the call.
        unsafe { back.resize_in_place(block, huge) };
        let want = if advised {
            HugePages::Prefer
        } else {
            HugePages::Avoid
        };
        assert_eq!(os.huge_pages(block), Some(want));
        back.free(block, new_class);
    }
}

/// Moves a dense block of `len` bytes of `class` into `to`, a fresh block whose object is to
/// be `size` bytes, as the heap's growing move does.
/// Spends the budget for kept grown blocks (`policy::kept_grown`) with a plain free of an
/// untouched block of a class of its own, so that moves and frees of grown blocks return their
/// pages, as they do in a process that has kept its fill. Returns the bytes left retained.
fn spend_kept_budget<O: Os>(back: &Back<'_, O>) -> usize {
    let class = large(kept_grown(back.space().plan().geometry()) + 1);
    let ballast = back.alloc(class, class.size()).unwrap().ptr;
    back.free(ballast, class);
    assert!(back.retained_large() >= class.size());
    back.retained()
}

fn move_in<O: Os>(back: &Back<'_, O>, class: Class, len: usize, to: NonNull<u8>, size: usize) {
    let from = back.alloc(class, len).unwrap().ptr;
    pattern(from.as_ptr(), len);
    back.resize(to, size);
    // SAFETY: both are live blocks that nothing else uses, and `to` is fresh.
    unsafe { back.move_large(from, to, len, true) };
    assert!(patterned(to.as_ptr(), len));
}

/// Within the budget for kept grown blocks, the slot a block moves out of and a grown block
/// freed with huge-page advice both stay resident, counted and, for the second, advised, as a
/// plain free leaves a slot; a block then allocated in the advised slot has the advice taken
/// back. Past the budget they go back to the OS, and a plain free never does.
#[test]
#[cfg_attr(miri, ignore)]
fn grown_blocks_are_kept_within_their_budget() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let page = Native::GEOMETRY.page_size();
    let budget = kept_grown(Native::GEOMETRY);
    // Slots more than twice what their blocks write, so that each is charged its block.
    let (old_class, new_class) = (large(192 << 10), large(512 << 10));
    assert!((96 << 10) + (192 << 10) <= budget);
    let purges = || os.calls(Call::Purge);
    let before = purges();
    // A block that grows by moving: the slot it leaves is kept, data and all.
    let old = back.alloc(old_class, 80 << 10).unwrap().ptr;
    pattern(old.as_ptr(), 80 << 10);
    let new = back.alloc(new_class, new_class.size()).unwrap().ptr;
    back.resize(new, 160 << 10);
    // SAFETY: both are live blocks that nothing else uses, and `new` is fresh.
    unsafe { back.move_large(old, new, 80 << 10, true) };
    assert!(patterned(new.as_ptr(), 80 << 10));
    assert_eq!(purges(), before, "nothing went back");
    assert!(
        patterned(old.as_ptr(), 80 << 10),
        "the old slot is resident"
    );
    // Charged what the block could have written, not its whole slot.
    let kept = back.retained_large();
    assert!((80 << 10..=(80 << 10) + page).contains(&kept), "{kept}");
    assert_eq!(os.huge_pages(new), Some(HugePages::Prefer));
    // Freed with its advice, it is kept too, advice and all, and charged its size in whole
    // huge pages.
    back.free(new, new_class);
    assert_eq!(purges(), before);
    assert_eq!(os.huge_pages(new), Some(HugePages::Prefer));
    assert_eq!(
        back.retained_large(),
        kept + (160usize << 10).next_multiple_of(huge)
    );
    // A block allocated in it rather than grown into it does not keep the advice.
    let again = back.alloc(new_class, new_class.size()).unwrap();
    assert_eq!(again.ptr, new);
    assert!(!again.zeroed);
    assert_eq!(os.huge_pages(new), Some(HugePages::Avoid));
    assert_eq!(back.retained_large(), kept);
    // Past the budget the same free returns the slot and its advice.
    move_in(&back, large(64 << 10), huge, again.ptr, 2 * huge);
    assert_eq!(os.huge_pages(new), Some(HugePages::Prefer));
    let ballast = spend_kept_budget(&back);
    let purged = purges();
    back.free(again.ptr, new_class);
    assert_eq!(purges(), purged + 1);
    assert_eq!(os.huge_pages(new), Some(HugePages::Avoid));
    assert_eq!(back.retained(), ballast);
    // A plain free keeps its slot whatever the count: this one takes the kept slot the move
    // left, and gives it back resident.
    let plain = back.alloc(old_class, old_class.size()).unwrap().ptr;
    assert_eq!(plain, old);
    assert_eq!(back.retained(), ballast - kept);
    back.free(plain, old_class);
    assert_eq!(purges(), purged + 1);
    assert_eq!(back.retained(), ballast - kept + old_class.size());
}

/// What a free of the large block at `ptr` of `class`, `size` bytes, should add to the
/// retained count: its size from the slot's stagger, in whole pages (whole huge pages if
/// the slot is advised), at most the slot. Read before the free, while the slot is held.
fn charge_of<O: Os>(back: &Back<'_, O>, class: Class, ptr: NonNull<u8>, size: usize) -> usize {
    use super::tree::Host as _;
    let host = back.host(class).unwrap();
    let layout = host.layout();
    let location = back.space().locate(ptr.as_ptr()).unwrap();
    let (index, _) = layout.block_at(location.offset as usize).unwrap();
    let huge = super::region::recorded(&host, location.region, index)
        .unwrap()
        .huge;
    let start = back
        .space()
        .slot(location.region, class, index)
        .addr()
        .get();
    let unit = if huge {
        layout.huge_pages() as usize * layout.page_size()
    } else {
        layout.page_size()
    };
    let end = (start + layout.stagger(index) + size).next_multiple_of(unit);
    let charge = (end - start).min(class.size());
    // A tail under half the slot is not told apart.
    if (class.size() - charge) * 2 < class.size() {
        class.size()
    } else {
        charge
    }
}

/// A large slot released resident is charged what its last tenant could have written, and a
/// claim and a purge take back exactly that, for a block at its size, one with room to grow,
/// a staggered one, one advised for huge pages and one shrunk in place. After a mixed run the
/// count is back at zero.
#[test]
#[cfg_attr(miri, ignore)]
fn a_large_slot_is_charged_what_its_tenant_could_write() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let page = Native::GEOMETRY.page_size();
    let huge = Native::GEOMETRY.huge_page_size();
    let class = large(256 << 10);
    // Each case leaves a held block, its size and whether to claim it back (else trim).
    let cases: [(&str, bool); 6] = [
        ("whole", true),
        ("headroom", false),
        ("staggered", true),
        ("advised", false),
        ("shrunk", true),
        ("grown", true),
    ];
    for (case, claim) in cases {
        assert_eq!(back.retained(), 0, "{case}");
        // A block held through the case, so that the claim below finds the case's slot.
        let mut skip = None;
        let (ptr, size) = match case {
            "whole" => (back.alloc(class, class.size()).unwrap().ptr, class.size()),
            "headroom" => (back.alloc(class, 40 << 10).unwrap().ptr, 40 << 10),
            "staggered" => {
                skip = Some(back.alloc_staggered(class, 1 << 10).unwrap().ptr);
                let ptr = back.alloc_staggered(class, 40 << 10).unwrap().ptr;
                assert_ne!(ptr.addr().get() % page, 0);
                (ptr, 40 << 10)
            }
            "advised" => {
                let ptr = back.alloc(class, class.size()).unwrap().ptr;
                move_in(&back, large(64 << 10), huge, ptr, 2 * huge);
                assert_eq!(state.os().huge_pages(ptr), Some(HugePages::Prefer));
                (ptr, 2 * huge)
            }
            "shrunk" => {
                let ptr = back.alloc(class, class.size()).unwrap().ptr;
                paint(ptr.as_ptr(), class.size(), 0x42);
                // SAFETY: the test holds the block and uses none of it during the call.
                unsafe { back.resize_in_place(ptr, 2 * page) };
                (ptr, 2 * page)
            }
            _ => {
                // A small tenant leaves the slot's tail clean; the next grows into it in place.
                let ptr = back.alloc(class, 8 << 10).unwrap().ptr;
                back.free(ptr, class);
                let ptr = back.alloc(class, 8 << 10).unwrap().ptr;
                assert_eq!(back.retained(), 0);
                // SAFETY: the test holds the block and uses none of it during the call.
                unsafe { back.resize_in_place(ptr, 200 << 10) };
                paint(ptr.as_ptr(), 200 << 10, 0x24);
                (ptr, 200 << 10)
            }
        };
        // The advised case's move kept the slot it came from, which is charged too.
        let base = back.retained();
        let charge = charge_of(&back, class, ptr, size);
        // The advised block's huge pages fill half its slot or more, by its stagger.
        if case != "advised" {
            let told = matches!(case, "headroom" | "staggered" | "shrunk");
            assert_eq!(charge < class.size(), told, "{case}");
        }
        back.free(ptr, class);
        assert_eq!(back.retained(), base + charge, "{case}");
        if claim {
            let again = back.alloc(class, class.size()).unwrap().ptr;
            assert_eq!(
                again,
                ptr.map_addr(|a| { core::num::NonZeroUsize::new(a.get() & !(page - 1)).unwrap() })
            );
            assert_eq!(back.retained(), base, "{case}");
            back.free(again, class);
            assert_eq!(back.retained(), base + class.size(), "{case}");
        }
        let held = back.retained();
        assert_eq!(
            back.trim(),
            held,
            "{case}: the purge takes back what was charged"
        );
        assert_eq!(back.retained(), 0, "{case}");
        if let Some(skip) = skip {
            let charge = charge_of(&back, class, skip, 1 << 10);
            back.free(skip, class);
            assert_eq!(back.trim(), charge);
            assert_eq!(back.retained(), 0);
        }
    }
}

/// A small tenant after a large one in a slot kept resident: the large tenant's pages past the
/// small one's extent stay resident but are marked clean, a known under-count. They are never
/// taken for zero, and the purge walk still returns all of the slot.
#[test]
#[cfg_attr(miri, ignore)]
fn a_small_tenant_after_a_large_one_is_under_counted_but_never_zero() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(256 << 10);
    let big = back.alloc(class, class.size()).unwrap().ptr;
    paint(big.as_ptr(), class.size(), 0x5e);
    back.free(big, class);
    let small = back.alloc(class, 40 << 10).unwrap();
    assert_eq!(small.ptr, big);
    assert!(!small.zeroed);
    back.free(small.ptr, class);
    assert_eq!(back.retained(), charge_of_free(40 << 10));
    let again = back.alloc(class, class.size()).unwrap();
    assert_eq!(again.ptr, big);
    assert!(!again.zeroed, "a resident tail is never reported zero");
    assert!(painted(
        big.as_ptr().wrapping_add(40 << 10),
        class.size() - (40 << 10),
        0x5e
    ));
    back.free(again.ptr, class);
    let small = back.alloc(class, 40 << 10).unwrap().ptr;
    back.free(small, class);
    back.trim();
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe { assert!(os.is_purged(big, class.size()), "the whole slot went back") };
    assert_eq!(back.retained(), 0);
}

/// The charge of a block of `size` bytes in an unadvised slot at index 0, which has no
/// stagger: its size in whole pages.
fn charge_of_free(size: usize) -> usize {
    size.next_multiple_of(Native::GEOMETRY.page_size())
}

/// In a slot advised for huge pages, a shrink returns its tail only in whole huge pages, and a
/// move returns the old block's pages piece by piece on huge-page boundaries even when the
/// slot does not start on one.
#[test]
#[cfg_attr(miri, ignore)]
fn an_advised_slot_is_purged_only_in_whole_huge_pages() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    spend_kept_budget(&back);
    let os = state.os();
    let page = Native::GEOMETRY.page_size();
    let huge = Native::GEOMETRY.huge_page_size();
    // 160 KiB slots: slot 1 starts 32 KiB past a huge-page boundary and holds two whole huge
    // pages, at 192 to 320 KiB of the region.
    let class = large(160 << 10);
    let first = back.alloc(class, page).unwrap().ptr;
    let slot = back.alloc(class, class.size()).unwrap().ptr;
    assert_eq!(slot.addr().get() - first.addr().get(), class.size());
    move_in(&back, large(64 << 10), huge, slot, 2 * huge);
    let interior = slot.map_addr(|a| a.saturating_add(huge / 2));
    assert_eq!(os.huge_pages(interior), Some(HugePages::Prefer));
    assert_eq!(
        os.huge_pages(slot),
        Some(HugePages::Avoid),
        "only whole huge pages"
    );
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(slot, class.size()) };
    paint(slot.as_ptr(), class.size(), 0x5a);
    // A shrink to the middle of the first huge page keeps all of it; the rest goes back.
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(slot, huge / 2 + 3 * page) };
    let past = interior.map_addr(|a| a.saturating_add(huge));
    assert!(painted(slot.as_ptr(), huge / 2 + huge, 0x5a));
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe { assert!(os.is_purged(past, class.size() - huge / 2 - huge)) };
    // A move of it: every page of the old slot goes back, the contents arrive.
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(slot, class.size()) };
    pattern(slot.as_ptr(), class.size());
    let to = back.alloc(large(512 << 10), 2 * class.size()).unwrap().ptr;
    let (mut pieces, purges) = (0, os.calls(Call::Purge));
    // SAFETY: both are live blocks that nothing else uses.
    unsafe { back.move_large_with(slot, to, class.size(), true, || pieces += 1) };
    assert!(patterned(to.as_ptr(), class.size()));
    // To the first boundary, then the two huge pages, each returned as it is copied.
    assert_eq!(pieces, 3);
    assert_eq!(os.calls(Call::Purge), purges + 3);
    // SAFETY: as above.
    unsafe { assert!(os.is_purged(slot, class.size())) };
    assert_eq!(os.huge_pages(interior), Some(HugePages::Avoid));
    back.free(first, class);
    back.free(to, large(512 << 10));
}

/// In an advised slot `purge_part` says whether all of the range went back, not just whether
/// the call succeeded: a move that is told everything went back marks the old slot zero, and a
/// trim that kept part of a huge page must not let it.
#[test]
#[cfg_attr(miri, ignore)]
fn a_trimmed_purge_of_an_advised_slot_says_it_kept_part() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    spend_kept_budget(&back);
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let class = large(256 << 10);
    let slot = back.alloc(class, class.size()).unwrap().ptr;
    move_in(&back, large(64 << 10), huge, slot, 2 * huge);
    assert_eq!(os.huge_pages(slot), Some(HugePages::Prefer));
    paint(slot.as_ptr(), class.size(), 0x21);
    let host = back.host(class).unwrap();
    let location = back.space().locate(slot.as_ptr()).unwrap();
    let index = back.space().plan().slot_at(location, class).unwrap();
    // SAFETY: the test holds the slot and needs none of its bytes.
    let (part, whole) = unsafe {
        (
            super::region::purge_part(&host, location.region, index, 0, huge + huge / 2),
            super::region::purge_part(&host, location.region, index, 2 * huge, huge),
        )
    };
    assert!(!part, "half a huge page stayed");
    assert!(painted(slot.as_ptr().wrapping_add(huge), huge / 2, 0x21));
    assert!(whole);
    back.free(slot, class);
}

/// A refused or partial advice leaves a correct heap: the flag is kept until the advice is
/// taken back, and the next holder of the slot tries again.
#[test]
#[cfg_attr(miri, ignore)]
fn refused_huge_page_advice_is_retried_by_the_next_holder() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    spend_kept_budget(&back);
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let class = large(256 << 10);
    let block = back.alloc(class, class.size()).unwrap().ptr;
    os.set_faults(Call::AdviseHuge, Faults::ALWAYS.partially());
    move_in(&back, large(64 << 10), huge, block, 2 * huge);
    // Half the range took the advice before the call failed.
    assert_eq!(os.huge_pages(block), Some(HugePages::Prefer));
    let calls = || os.calls(Call::AdviseHuge);
    let before = calls();
    back.free(block, class);
    assert_eq!(calls(), before + 1, "the free tried to take it back");
    // The next holder tries again, and so does its free, until the OS accepts.
    let again = back.alloc(class, class.size()).unwrap();
    assert_eq!(again.ptr, block);
    assert!(again.zeroed && painted(block.as_ptr(), class.size(), 0));
    assert_eq!(calls(), before + 2);
    back.free(again.ptr, class);
    assert_eq!(calls(), before + 3);
    os.set_faults(Call::AdviseHuge, Faults::NONE);
    let third = back.alloc(class, class.size()).unwrap();
    assert_eq!(third.ptr, block);
    assert_eq!(calls(), before + 4);
    let last = block.map_addr(|a| a.saturating_add(class.size() - 1));
    assert_eq!(os.huge_pages(block), Some(HugePages::Avoid));
    assert_eq!(os.huge_pages(last), Some(HugePages::Avoid));
    back.free(third.ptr, class);
    assert_eq!(calls(), before + 4, "taken back for good");
}

/// A block written sparsely stays sparse when it moves: the copy skips its all-zero pages, so
/// no more is resident after the move than before, plus a piece, and the contents arrive.
/// Into a destination that is not known to be zero, the move first purges the part of it under
/// each piece's zero pages, so the skipped pages read as zero there too.
#[test]
#[cfg_attr(miri, ignore)]
fn a_sparse_block_stays_sparse_when_it_moves() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    spend_kept_budget(&back);
    let os = state.os();
    let page = Native::GEOMETRY.page_size();
    let piece = move_piece(Native::GEOMETRY);
    let (old_class, new_class) = (large(512 << 10), large(1 << 20));
    let len = old_class.size();
    // One byte in every fourth page.
    let marks: Vec<usize> = (0..len).step_by(4 * page).map(|o| o + 3).collect();
    let write = |at: NonNull<u8>| {
        for (i, &o) in marks.iter().enumerate() {
            // SAFETY: inside a live block the test owns.
            unsafe { at.as_ptr().add(o).write(i as u8 | 1) };
        }
    };
    let arrived = |at: NonNull<u8>| {
        // SAFETY: as above.
        let bytes = unsafe { core::slice::from_raw_parts(at.as_ptr(), len) };
        bytes
            .iter()
            .enumerate()
            .all(|(o, &b)| match marks.binary_search(&o) {
                Ok(i) => b == i as u8 | 1,
                Err(_) => b == 0,
            })
    };
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    let resident = || unsafe { os.resident_bytes() };
    for zeroed in [true, false] {
        let old = back.alloc(old_class, len).unwrap().ptr;
        write(old);
        let new = back.alloc(new_class, new_class.size()).unwrap();
        if !zeroed {
            // A destination an earlier block left dirty.
            paint(new.ptr.as_ptr(), new_class.size(), 0x77);
        }
        // The second round reuses the first's slot, which its free kept resident.
        assert_eq!(new.zeroed, zeroed, "a fresh slot, then the kept one");
        let before = resident();
        let purges = os.calls(Call::Purge) as usize;
        // SAFETY: both are live blocks that nothing else uses; `new` reads as zero when said.
        unsafe { back.move_large(old, new.ptr, len, zeroed) };
        assert!(arrived(new.ptr), "zeroed {zeroed}");
        let sparse = marks.len() * page;
        let dirty_rest = if zeroed { 0 } else { new_class.size() - len };
        assert!(
            resident() <= sparse + dirty_rest + piece,
            "{} bytes resident after moving {sparse} bytes of pages (from {before})",
            resident()
        );
        // One purge a piece, as for a dense block (the block fills its slot, so nothing is
        // left after the last), and, when the destination was not known to be zero, one of
        // its part under each piece's zero pages.
        let expected = len / piece * (1 + usize::from(!zeroed));
        assert_eq!(os.calls(Call::Purge) as usize, purges + expected);
        back.free(new.ptr, new_class);
    }
}

/// Review finding: `move_large_with` ignores the result of the purge that is meant to make a
/// dirty destination read as zero, and then skips the source's zero pages all the same. Where
/// `madvise(MADV_DONTNEED)` fails, as it does on locked memory (`mlockall`), the pages it skipped
/// keep what the destination's last owner left there.
#[test]
#[cfg_attr(miri, ignore)]
fn a_block_moved_into_a_dirty_slot_arrives_whole_when_purges_fail() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    spend_kept_budget(&back);
    let os = state.os();
    let page = Native::GEOMETRY.page_size();
    let (old_class, new_class) = (large(512 << 10), large(1 << 20));
    let len = old_class.size();
    let old = back.alloc(old_class, len).unwrap().ptr;
    // One byte in every fourth page; the rest of the block is zero.
    for at in (0..len).step_by(4 * page) {
        // SAFETY: inside a live block the test owns.
        unsafe { old.as_ptr().add(at + 3).write(1) };
    }
    let new = back.alloc(new_class, new_class.size()).unwrap();
    paint(new.ptr.as_ptr(), new_class.size(), 0x77);
    os.set_faults(Call::Purge, Faults::ALWAYS);
    // SAFETY: both are live blocks that nothing else uses; `new` is not said to be zero.
    unsafe { back.move_large(old, new.ptr, len, false) };
    os.set_faults(Call::Purge, Faults::NONE);
    // SAFETY: a live block the test owns.
    let bytes = unsafe { core::slice::from_raw_parts(new.ptr.as_ptr(), len) };
    let stale = bytes.iter().filter(|&&b| b == 0x77).count();
    assert_eq!(
        stale, 0,
        "bytes of the destination's last owner in the moved block"
    );
    back.free(new.ptr, new_class);
}

/// A block written densely but for a hole of zero pages across a piece boundary moves as one
/// copy per dense piece and leaves the hole out. A destination not known to be zero is purged
/// only under the hole, once in each piece the hole touches, and never where the block is
/// dense; a staggered destination, whose pages do not line up with the source's, is purged in
/// the whole pages under the hole and gets the zeros at its edges copied.
#[test]
#[cfg_attr(miri, ignore)]
fn a_dense_block_with_a_hole_moves_whole_and_keeps_the_hole() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    spend_kept_budget(&back);
    let os = state.os();
    let page = Native::GEOMETRY.page_size();
    let piece = move_piece(Native::GEOMETRY);
    // Two slots of the new class to a region, so the second is staggered.
    let (old_class, new_class) = (large(256 << 10), large(512 << 10));
    let len = old_class.size();
    assert!(len >= 4 * piece);
    // Three pages before the third piece and two after its start.
    let hole = 2 * piece - 3 * page..2 * piece + 2 * page;
    let arrived = |at: NonNull<u8>| {
        // SAFETY: a live block the test owns.
        let bytes = unsafe { core::slice::from_raw_parts(at.as_ptr(), len) };
        bytes.iter().enumerate().all(|(o, &b)| {
            if hole.contains(&o) {
                b == 0
            } else {
                b == (o / 7 + o / 4096) as u8 | 1
            }
        })
    };
    for (dirty, staggered) in [(false, false), (true, false), (true, true)] {
        let old = back.alloc(old_class, len).unwrap().ptr;
        pattern(old.as_ptr(), len);
        // SAFETY: inside the live block.
        unsafe { old.as_ptr().add(hole.start).write_bytes(0, hole.len()) };
        let skip = back.alloc(new_class, 1).unwrap().ptr;
        let new = if staggered {
            back.alloc_staggered(new_class, len).unwrap()
        } else {
            back.alloc(new_class, len).unwrap()
        };
        assert_eq!(new.ptr.addr().get() % page != 0, staggered);
        assert!(dirty || new.zeroed);
        if dirty {
            let start = new
                .ptr
                .map_addr(|a| core::num::NonZeroUsize::new(a.get() & !(page - 1)).unwrap());
            paint(start.as_ptr(), new_class.size(), 0x77);
        }
        let purges = os.calls(Call::Purge) as usize;
        // SAFETY: both are live blocks that nothing else uses; `new` reads as zero unless
        // painted.
        unsafe { back.move_large(old, new.ptr, len, !dirty) };
        assert!(arrived(new.ptr), "dirty {dirty} staggered {staggered}");
        // One purge for each piece of the old block, one of the destination for each of the
        // two pieces the hole touches, and none for the dense pieces.
        assert_eq!(
            os.calls(Call::Purge) as usize,
            purges + len / piece + if dirty { 2 } else { 0 },
            "dirty {dirty} staggered {staggered}"
        );
        if dirty {
            // The whole destination pages under the hole went back and were not written.
            let t = new.ptr.addr().get();
            let lo = (t + hole.start).next_multiple_of(page);
            let hi = (t + hole.end) & !(page - 1);
            let under = new.ptr.map_addr(|a| a.saturating_add(lo - t));
            // SAFETY: no other thread runs, and the back end and this test store only integers.
            unsafe { assert!(os.is_purged(under, hi - lo)) };
        }
        back.free_by_address(new.ptr);
        back.free_by_address(skip);
    }
}

/// When the purge of a dirty destination under a hole fails, as every purge does on locked
/// memory, the hole is copied instead, so none of the destination's old bytes survive; the
/// move stops trying to purge the destination, and still returns the old block's pages.
#[test]
#[cfg_attr(miri, ignore)]
fn a_hole_is_copied_when_the_destinations_purge_fails() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let ballast = spend_kept_budget(&back);
    let os = state.os();
    let page = Native::GEOMETRY.page_size();
    let piece = move_piece(Native::GEOMETRY);
    let (old_class, new_class) = (large(512 << 10), large(1 << 20));
    let len = old_class.size();
    // A hole in the third piece and one in the fourth.
    let holes = [
        2 * piece + page..2 * piece + 4 * page,
        3 * piece..3 * piece + 2 * page,
    ];
    let old = back.alloc(old_class, len).unwrap().ptr;
    pattern(old.as_ptr(), len);
    for hole in &holes {
        // SAFETY: inside the live block.
        unsafe { old.as_ptr().add(hole.start).write_bytes(0, hole.len()) };
    }
    let new = back.alloc(new_class, new_class.size()).unwrap();
    paint(new.ptr.as_ptr(), new_class.size(), 0x77);
    let purges = os.calls(Call::Purge) as usize;
    // The first two pieces are dense and go back; the destination's purge under the first
    // hole is the third call.
    os.set_faults(Call::Purge, Faults::once_after(2));
    // SAFETY: both are live blocks that nothing else uses; `new` is not said to be zero.
    unsafe { back.move_large(old, new.ptr, len, false) };
    os.set_faults(Call::Purge, Faults::NONE);
    // SAFETY: a live block the test owns.
    let bytes = unsafe { core::slice::from_raw_parts(new.ptr.as_ptr(), len) };
    assert!(bytes.iter().enumerate().all(|(o, &b)| {
        if holes.iter().any(|h| h.contains(&o)) {
            b == 0
        } else {
            b == (o / 7 + o / 4096) as u8 | 1
        }
    }));
    // Every piece went back, and one purge of the destination was tried.
    assert_eq!(os.calls(Call::Purge) as usize, purges + len / piece + 1);
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe { assert!(os.is_purged(old, len)) };
    assert_eq!(back.retained(), ballast);
    back.free(new.ptr, new_class);
}

/// Large blocks of one class start at their slots' staggers: different multiples of a cache
/// line, within a page. Everything that maps a pointer back to its slot agrees: lookup, size,
/// usable size, room, resize, and the free, which aborts on any other offset into the slot.
#[test]
#[cfg_attr(miri, ignore)]
fn staggered_blocks_start_at_different_cache_lines_and_free_by_address() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let page = Native::GEOMETRY.page_size();
    let class = large(64 << 10);
    let size = class.size() - STAGGER_ROOM;
    let blocks: Vec<_> = (0..8)
        .map(|_| back.alloc_staggered(class, size).unwrap().ptr)
        .collect();
    let mut offsets: Vec<usize> = blocks.iter().map(|b| b.addr().get() % page).collect();
    assert!(offsets.iter().all(|o| o % STAGGER_STEP == 0));
    offsets.sort_unstable();
    offsets.dedup();
    assert_eq!(offsets.len(), blocks.len(), "each slot its own stagger");
    for &b in &blocks {
        let into = b.addr().get() % page;
        assert_eq!(back.lookup(b.as_ptr()), Some(Kind::Slot(class)));
        assert_eq!(back.size(b.as_ptr()), Some(size));
        assert_eq!(back.room(b.as_ptr()), Some(class.size() - into));
        let usable = back.usable_size(b.as_ptr()).unwrap();
        assert!(usable >= size && into + usable <= class.size());
        assert_eq!((b.addr().get() + usable) % page, 0, "to a page boundary");
        paint(b.as_ptr(), usable, 0x3c);
    }
    // Shrunk in place, the tail goes back from the page the new end is in.
    let b = blocks[3];
    let into = b.addr().get() % page;
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(b, page) };
    let end = (into + page).next_multiple_of(page) - into;
    assert!(painted(b.as_ptr(), end, 0x3c));
    let tail = b.map_addr(|a| a.saturating_add(end));
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe { assert!(state.os().is_purged(tail, class.size() - into - end)) };
    for &b in &blocks {
        back.free_by_address(b);
    }
    // A slot handed out unstaggered still frees from its start.
    let plain = back.alloc(class, class.size()).unwrap().ptr;
    assert_eq!(back.room(plain.as_ptr()), Some(class.size()));
    back.free_by_address(plain);
}

#[test]
#[should_panic(expected = "not the start")]
fn a_pointer_between_a_slot_and_its_stagger_is_not_a_block() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = large(64 << 10);
    let _first = back.alloc_staggered(class, 1 << 10).unwrap().ptr;
    // The second slot's stagger is one step; half a step is no block's start.
    let second = back.alloc_staggered(class, 1 << 10).unwrap().ptr;
    assert_eq!(
        second.addr().get() % Native::GEOMETRY.page_size(),
        STAGGER_STEP
    );
    let between = second.map_addr(|a| a.saturating_add(STAGGER_STEP / 2));
    let between =
        between.map_addr(|a| core::num::NonZeroUsize::new(a.get() - STAGGER_STEP).unwrap());
    back.free_by_address(between);
}

/// A staggered block is not a multiple of its slot size from the region's base, so a free
/// takes the launder's full check rather than its fast one; it still frees exactly its slot,
/// which the next allocations get back.
#[test]
#[cfg_attr(miri, ignore)]
fn freeing_staggered_blocks_frees_their_slots() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = large(64 << 10);
    let page = Native::GEOMETRY.page_size();
    let starts = |blocks: &[NonNull<u8>]| -> Vec<usize> {
        blocks
            .iter()
            .map(|b| b.addr().get() & !(page - 1))
            .collect()
    };
    let blocks: Vec<_> = (0..6)
        .map(|_| back.alloc_staggered(class, 1 << 10).unwrap().ptr)
        .collect();
    assert!(
        blocks[1..].iter().all(|b| b.addr().get() % page != 0),
        "staggered"
    );
    for &b in &blocks {
        back.free_by_address(b);
    }
    let again: Vec<_> = (0..6)
        .map(|_| back.alloc(class, 1 << 10).unwrap().ptr)
        .collect();
    assert_eq!(starts(&again), starts(&blocks));
}

/// A whole magazine's worth of dense slots in one launder, with one of them twice: the held
/// word's count catches it.
#[test]
#[should_panic(expected = "freed twice in one batch")]
fn a_slot_twice_in_a_dense_magazine_aborts() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(32);
    let slots = fill(&back, class, 254);
    let mut batch = slots.clone();
    batch[200] = slots[199];
    // SAFETY: the test holds these slots, or passes a bad one to see the abort.
    unsafe { back.launder(class, &batch) };
}

/// A fill into a magazine's slots puts the lowest address last, where the first pop takes it,
/// for freed slots and carved ones alike.
#[test]
fn a_descending_fill_puts_the_lowest_address_on_top() {
    use super::Descending;
    use core::sync::atomic::AtomicPtr;
    use core::sync::atomic::Ordering::Relaxed;
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(48);
    let held = fill(&back, class, 300);
    // Free a scattered few, so the next fill takes them before it carves.
    let freed: Vec<*mut u8> = held.iter().step_by(7).copied().collect();
    // SAFETY: the test holds these slots and uses none of them.
    unsafe { back.launder(class, &freed) };
    let slots: Vec<AtomicPtr<u8>> = (0..126)
        .map(|_| AtomicPtr::new(core::ptr::null_mut()))
        .collect();
    assert_eq!(back.fill(class, &mut Descending(&slots)), slots.len());
    let popped: Vec<*mut u8> = slots.iter().rev().map(|s| s.load(Relaxed)).collect();
    assert!(popped.is_sorted(), "pops ascend");
    assert_eq!(popped[..freed.len()], freed[..], "freed slots first");
    // A slice of pointers is filled in ascending order from its start.
    let mut few = [core::ptr::null_mut(); 3];
    assert_eq!(back.fill(class, &mut few[..]), 3);
    assert!(few.is_sorted());
}

/// A staggered block moving to another staggered block, densely and sparsely: the contents
/// arrive, and every page of the old slot goes back, so it is handed out again as zero.
#[test]
#[cfg_attr(miri, ignore)]
fn a_staggered_block_moves_and_returns_its_whole_slot() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    spend_kept_budget(&back);
    let os = state.os();
    let (old_class, new_class) = (large(512 << 10), large(1 << 20));
    for sparse in [false, true] {
        // Take a slot first, so the next ones are staggered.
        let skip = back.alloc_staggered(old_class, 1 << 10).unwrap().ptr;
        let len = old_class.size() - STAGGER_ROOM;
        let old = back.alloc_staggered(old_class, len).unwrap().ptr;
        assert_ne!(old.addr().get() % Native::GEOMETRY.page_size(), 0);
        if sparse {
            for o in (0..len).step_by(5 * 4096) {
                // SAFETY: inside the block.
                unsafe { old.as_ptr().add(o).write(7) };
            }
        } else {
            pattern(old.as_ptr(), len);
        }
        let new = back.alloc_staggered(new_class, len).unwrap();
        // SAFETY: both are live blocks that nothing else uses; `new` reads as zero if said.
        unsafe { back.move_large(old, new.ptr, len, new.zeroed) };
        if sparse {
            // SAFETY: inside the block.
            let bytes = unsafe { core::slice::from_raw_parts(new.ptr.as_ptr(), len) };
            assert!(
                bytes
                    .iter()
                    .enumerate()
                    .all(|(o, &b)| b == if o % (5 * 4096) == 0 { 7 } else { 0 })
            );
        } else {
            assert!(patterned(new.ptr.as_ptr(), len));
        }
        let slot = back.space().slot(
            back.space().locate(old.as_ptr()).unwrap().region,
            old_class,
            back.space().locate(old.as_ptr()).unwrap().slot(old_class),
        );
        // SAFETY: as above.
        unsafe { assert!(os.is_purged(slot, old_class.size())) };
        let again = back.alloc(old_class, old_class.size()).unwrap();
        assert_eq!(again.ptr, slot);
        assert!(again.zeroed && painted(slot.as_ptr(), old_class.size(), 0));
        back.free(again.ptr, old_class);
        back.free_by_address(new.ptr);
        back.free_by_address(skip);
    }
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
    unsafe { back.move_large_with(old, new, len, true, || peak = peak.max(resident())) };
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
        Grow::unpack(host.grow(region).load()).carved
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
        let victims = fill(&back, low, 512);
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
            // SAFETY: the test holds these slots, or passes a bad one to see the abort.
            unsafe { back.launder(class, chunk) };
        }
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(low, &victims) };
        assert!(back.retained() < retained_cap(Wide::GEOMETRY));
        for _ in 0..10 {
            os.advance_clock(DECAY_TICKS);
            beat(&back);
        }
        let laps = state.walk.laps.load(core::sync::atomic::Ordering::Relaxed);
        let at = state
            .walk
            .purge_at
            .load(core::sync::atomic::Ordering::Relaxed);
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
        let slots = fill(&back, class, page / 16);
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, &slots) };
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

    /// Finding count-drifts-up, across threads. A race, so it is evidence when it fails and
    /// none when it passes: many threads allocate and free slots of one large class. When one
    /// thread's claim lowers the count between another's `release` and `retain`, a count that
    /// stopped at zero would lose the subtraction and keep the addition, and after a trim it
    /// would say bytes are resident with nothing free.
    #[test]
    fn racing_large_frees_and_claims_leave_no_count_behind() {
        let state = state::<Native>();
        state.get_or_reserve().unwrap();
        let state = &*state;
        let class = large(512 << 10);
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
            back.retained(),
            0,
            "every slot is free and purged, yet the count says this much is resident"
        );
    }

    /// Finding cap-has-no-hysteresis. When the cap acted at the free, a count that hovered at
    /// the cap fired on every launder that crossed upward, and each walked the whole
    /// fragmented heap for nothing. Now frees and reuse never walk at all.
    #[test]
    fn a_count_hovering_at_the_cap_never_walks_the_heap() {
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
            // SAFETY: the test holds these slots, or passes a bad one to see the abort.
            unsafe { back.launder(class, chunk) };
        }
        assert_eq!(back.retained(), cap - 32 * 1024);
        let laps = || state.walk.laps.load(core::sync::atomic::Ordering::Relaxed);
        let before = laps();
        // The program frees 64 slots and takes 64 back, over and over, so the count moves
        // between 32 KiB under the cap and 32 KiB over it.
        for _ in 0..50 {
            // SAFETY: the test holds these slots, or passes a bad one to see the abort.
            unsafe { back.launder(class, &extra) };
            assert!(back.retained() > cap);
            extra = fill(&back, class, 64);
            assert!(back.retained() <= cap);
        }
        assert_eq!(os.calls(Call::Purge), 0, "nothing was purgeable");
        assert_eq!(
            laps(),
            before,
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
        let slots = fill(&back, class, total / 1024);
        for &p in &slots {
            paint(p, 1024, 5);
        }
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, &slots) };
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
        let slots = fill(&back, class, total / class.size());
        for &p in &slots {
            paint(p, class.size(), 5);
        }
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, &slots) };
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
        let slots = fill(&back, class, 256);
        for &p in &slots {
            paint(p, 1024, 5);
        }
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, &slots) };
        // The pass began just now: one that stays unpublished for an interval is another
        // matter (`a_pass_whose_starter_vanished_before_publishing_ends_some_day`).
        state.os().advance_clock(7 * DECAY_TICKS);
        let started = (7 * DECAY_TICKS) | 1;
        let stale = Pass {
            tag: Pass::tag_of(started) ^ 1,
            lap: 0,
            pages: 0,
        };
        state.walk.pass.store(stale.pack(), Relaxed);
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
            lap: state.walk.laps.load(Relaxed) as u16,
            pages: 16,
        };
        state.walk.pass.store(goal.pack(), Relaxed);
        beat(&back);
        assert!(!pass_under_way(&state));
        // One call for a whole huge page, or one a page below the huge-page part.
        assert!(state.os().calls(Call::Purge) > 0);
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
        let slots = fill(&back, class, 256);
        for &p in &slots {
            paint(p, 1024, 5);
        }
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, &slots) };
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

    /// Finding cap-purge-stops-at-the-lap-count. The cap's purge starts wherever the shared
    /// walk was left. A purge that stopped when the lap count changed would, from near the
    /// bottom, look at the few regions below, end the lap, and stop without having seen the
    /// regions that hold the freed memory. Here the walk was left at region 0 (set by hand; a
    /// decay pass that met its goal there leaves it so), the program frees more than the cap,
    /// all of it purgeable, in the regions above, and another class then grows.
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
        state.walk.purge_at.store(1, Relaxed);
        for chunk in slots.chunks_mut(64) {
            // SAFETY: the test holds these slots, or passes a bad one to see the abort.
            unsafe { back.launder(class, chunk) };
        }
        let step = growth_step(Standard::GEOMETRY);
        let _grown = fill(&back, small(512), 2 * step / 512);
        assert!(
            os.calls(Call::Purge) > 0 && back.retained() <= cap,
            "{} bytes are free and wholly purgeable, the cap is {cap}, and the cap's purge \
             returned nothing",
            back.retained()
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
            // SAFETY: the test holds these slots, or passes a bad one to see the abort.
            unsafe { back.launder(class, chunk) };
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

    /// A batch of distinct slots in one of the shapes a launder meets, drawn from `pool`.
    fn batch(rng: &mut Rng, pool: &[*mut u8]) -> Vec<*mut u8> {
        let len = 1 + rng.below(254);
        let mut picked = BTreeSet::new();
        match rng.below(4) {
            // A dense run, ascending or descending.
            0 => {
                let start = rng.below(pool.len() - len);
                picked.extend(start..start + len);
            }
            // Shuffled among a few neighbouring words.
            1 => {
                let start = rng.below(pool.len() - 600);
                while picked.len() < len {
                    picked.insert(start + rng.below(600));
                }
            }
            // Scattered over the pool, both regions.
            2 => {
                while picked.len() < len {
                    picked.insert(rng.below(pool.len()));
                }
            }
            // A few words, far apart, visited over and over.
            _ => {
                let bases: Vec<usize> = (0..6).map(|_| rng.below(pool.len() - 64)).collect();
                while picked.len() < len.min(200) {
                    picked.insert(bases[rng.below(6)] + rng.below(64));
                }
            }
        }
        let mut order: Vec<usize> = picked.into_iter().collect();
        match rng.below(3) {
            0 => {}
            1 => order.reverse(),
            _ => {
                for i in (1..order.len()).rev() {
                    order.swap(i, rng.below(i + 1));
                }
            }
        }
        order.into_iter().map(|i| pool[i]).collect()
    }

    /// Verification of the unsorted launder and the resumed fill, single-threaded: whatever
    /// the order and shape of a batch of distinct slots, the launder aborts on none of them,
    /// every slot is visible afterwards, and a fill takes them back lowest first.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn launders_in_any_order_free_exactly_their_slots() {
        let state = state::<Standard>();
        let back = state.get_or_reserve().unwrap();
        let class = small(8);
        let pool = fill(&back, class, 40_000);
        assert_eq!(pool.len(), 40_000);
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for round in 0..400 {
            let batch = batch(&mut rng, &pool);
            // SAFETY: the test holds every slot of the pool.
            unsafe { back.launder(class, &batch) };
            let mut want = batch.clone();
            want.sort_unstable();
            assert_eq!(fill(&back, class, batch.len()), want, "round {round}");
        }
    }

    /// Verification of the batched walk up the summary tree under racing fills and launders:
    /// once every thread has finished, no free slot sits under a clear summary bit. Threads
    /// fill and launder scattered batches; at the end a fill of as many slots as were ever
    /// handed out must get exactly those back, carving nothing new.
    fn after_racing_launders_and_fills_every_free_slot_is_found(seeds: u64) {
        for seed in 0..seeds {
            let state = state::<Standard>();
            state.get_or_reserve().unwrap();
            let state = &*state;
            let class = small(8);
            let seen: Mutex<BTreeSet<usize>> = Mutex::new(BTreeSet::new());
            thread::scope(|scope| {
                for t in 0..8u64 {
                    let seen = &seen;
                    scope.spawn(move || {
                        let back = state.get().unwrap();
                        let mut rng = Rng(0x5151_0000 + seed * 64 + t);
                        let mut held: Vec<*mut u8> = Vec::new();
                        for _ in 0..300 {
                            held.extend(fill(&back, class, 1 + rng.below(254)));
                            seen.lock().unwrap().extend(held.iter().map(|p| p.addr()));
                            for i in (1..held.len()).rev() {
                                held.swap(i, rng.below(i + 1));
                            }
                            let keep = rng.below(held.len() + 1);
                            let give = held.split_off(keep);
                            for chunk in give.chunks(64) {
                                // SAFETY: this thread holds these slots.
                                unsafe { back.launder(class, chunk) };
                            }
                        }
                        // SAFETY: this thread holds these slots.
                        unsafe { back.launder(class, &held) };
                    });
                }
            });
            let back = state.get().unwrap();
            let seen = seen.into_inner().unwrap();
            let all: BTreeSet<usize> = fill(&back, class, seen.len())
                .iter()
                .map(|p| p.addr())
                .collect();
            assert_eq!(all, seen, "seed {seed}: a free slot was not found");
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn after_racing_launders_and_fills_every_free_slot_is_found_quickly() {
        after_racing_launders_and_fills_every_free_slot_is_found(4);
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn after_racing_launders_and_fills_every_free_slot_is_found_fully() {
        after_racing_launders_and_fills_every_free_slot_is_found(20);
    }

    /// The same batches with one slot repeated anywhere later in the batch: with no other
    /// thread to claim the slot in between, every one must abort, within a part or across
    /// parts.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_slot_twice_anywhere_in_a_batch_aborts_when_nothing_claims_it() {
        let mut rng = Rng(0x1234_5678_9abc_def1);
        for round in 0..150 {
            let state = state::<Standard>();
            let back = state.get_or_reserve().unwrap();
            let class = small(8);
            let pool = fill(&back, class, 40_000);
            let mut batch = batch(&mut rng, &pool);
            let first = rng.below(batch.len());
            let again = first + 1 + rng.below(batch.len() - first);
            batch.insert(again, batch[first]);
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // SAFETY: the test holds every slot of the pool; the repeat is the point.
                unsafe { back.launder(class, &batch) };
            }));
            assert!(
                caught.is_err(),
                "round {round}: slot {first} again at {again} of {} was not caught",
                batch.len()
            );
        }
    }
}

/// Review of the move of the region headers into the back end's state, with the leaf bitmap
/// last in the block (`host::RegionHeader`, `layout`).
mod review_headers {
    use super::super::layout::{Layout, WORD_BYTES};
    use super::super::tree::Host;
    use super::super::word::Word;
    use super::*;
    use crate::space::Geometry;
    use crate::stats::{Region, Visitor};

    /// On the real OS, in the design's geometry, where the pages a block costs are real.
    #[cfg(not(miri))]
    mod resident {
        use super::*;
        use crate::os::linux::LinuxOs;
        use std::fs::File;
        use std::os::unix::fs::FileExt;

        /// Which pages of the `len` bytes at `start` this process has touched, as page numbers
        /// from `start`: its own pages, and pages that a read mapped to the kernel's shared
        /// zero page, which cost no memory but do need a page-table entry.
        fn touched(start: *const u8, len: usize) -> (Vec<usize>, Vec<usize>) {
            const PRESENT: u64 = 1 << 63;
            const EXCLUSIVE: u64 = 1 << 56;
            let page = Geometry::DESIGN.page_size();
            let pagemap = File::open("/proc/self/pagemap").unwrap();
            let mut entries = std::vec![0u8; len / page * 8];
            pagemap
                .read_exact_at(&mut entries, (start.addr() / page * 8) as u64)
                .unwrap();
            let (mut own, mut zero) = (Vec::new(), Vec::new());
            for (i, &entry) in entries.as_chunks::<8>().0.iter().enumerate() {
                let entry = u64::from_ne_bytes(entry);
                if entry & PRESENT == 0 {
                    continue;
                }
                if entry & EXCLUSIVE != 0 {
                    own.push(i);
                } else {
                    zero.push(i);
                }
            }
            (own, zero)
        }

        /// The block of the region `at` lies in, as the pages of it this process has touched.
        fn block_touched(
            back: &Back<'_, LinuxOs>,
            class: Class,
            at: *mut u8,
        ) -> (Vec<usize>, Vec<usize>) {
            let region = back.space().locate(at).unwrap().region;
            let len = back.host(class).unwrap().layout().block_bytes();
            touched(back.space().metadata(region).as_ptr(), len)
        }

        /// What the change is for: a small class whose slots are only ever carved, used and
        /// kept leaves every page of its region's metadata block untouched, read or written;
        /// and one whose slots go to the bitmaps and come back touches at most three pages of
        /// its own, with every page it touches at all in the block's first 2 MiB, under one
        /// page of page tables.
        ///
        /// Large classes are not in this: every large allocation writes its slot's size word,
        /// which is in the block.
        #[test]
        fn a_small_class_touches_its_block_only_once_its_slots_reach_the_bitmaps() {
            let state = Box::new(State::new(LinuxOs));
            let back = state.get_or_reserve().unwrap();
            let huge = Geometry::DESIGN.huge_page_size() / Geometry::DESIGN.page_size();
            let sizes = [8, 16, 48, 256, 1024, 4096, 32 << 10];
            for size in sizes {
                let class = small(size);
                let slots = fill(&back, class, 64);
                assert_eq!(slots.len(), 64);
                for &p in &slots {
                    paint(p, size, 0x5a);
                }
                // The paths a class takes while nothing of it is free: a single-slot
                // allocation, lookups, a heartbeat and a trim.
                let one = back.alloc(class.class(), size).unwrap();
                assert_eq!(back.usable_size(one.ptr.as_ptr()), Some(size));
                back.heartbeat();
                back.trim();
                let (own, zero) = block_touched(&back, class.class(), slots[0]);
                assert!(
                    own.is_empty() && zero.is_empty(),
                    "{size}: a class that only carves touched pages {own:?} and read {zero:?}"
                );
            }
            for size in sizes {
                let class = small(size);
                let slots = fill(&back, class, 64);
                // SAFETY: the test holds these slots and gives them up.
                unsafe { back.launder(class, &slots) };
                assert_eq!(fill(&back, class, 64), slots);
                let (own, zero) = block_touched(&back, class.class(), slots[0]);
                assert!(!own.is_empty() && own.len() <= 3, "{size}: touched {own:?}");
                assert!(
                    own.iter().chain(&zero).all(|&page| page < huge),
                    "{size}: touched {own:?} and read {zero:?}"
                );
            }
        }
    }

    /// In the design's geometry, at every region size and for every class that fits, the first
    /// word of every part of the block lies in the block's first 2 MiB. A small class's leaf
    /// bitmap starts a page and is all that is not committed at assignment; a large class's
    /// block, header first, is committed whole.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn every_design_layout_starts_its_parts_in_the_first_huge_page() {
        let geometry = Geometry::DESIGN;
        let (page, huge) = (geometry.page_size(), geometry.huge_page_size());
        let mut plans = 0;
        for shift in 0..=64 {
            let Some(plan) = geometry.plan(shift) else {
                continue;
            };
            plans += 1;
            for class in Class::all().filter(|&c| plan.fits_region(c)) {
                let layout = Layout::new(plan, class).unwrap();
                let leaf = layout.tree_word(0, 0) as usize * WORD_BYTES;
                if class.is_small() {
                    assert_eq!(leaf % page, 0, "{shift} {class:?}");
                    assert_eq!(layout.fixed_commit(), (0, leaf), "{shift} {class:?}");
                    assert_eq!(layout.leaf_commit(0), leaf, "{shift} {class:?}");
                    assert_eq!(layout.header_word(), None);
                } else {
                    let whole = layout.block_bytes();
                    assert_eq!(layout.fixed_commit(), (0, whole), "{shift} {class:?}");
                    assert_eq!(layout.leaf_commit(0), whole, "{shift} {class:?}");
                    assert_eq!(layout.header_word(), Some(0));
                }
                let firsts = (0..=layout.height())
                    .map(|level| layout.tree_word(level, 0))
                    .chain([layout.clean_word(0)])
                    .chain(layout.zero_word(0))
                    .chain(layout.size_word(0));
                for word in firsts {
                    assert!((word as usize * WORD_BYTES) < huge, "{shift} {class:?}");
                }
            }
        }
        assert!(plans > 1);
    }

    #[derive(Default)]
    struct Seen {
        regions: Vec<(Class, usize, usize)>,
        free: usize,
    }

    impl Visitor for Seen {
        fn region(&mut self, region: &Region) {
            self.regions
                .push((region.class, region.carved, region.committed));
        }
        fn free(&mut self, _: Class, _: *const u8, _: usize) {
            self.free += 1;
        }
    }

    /// A region that carries `class`'s tag with an all-zero header and a block nothing has
    /// written: `assign` published it, and the OS refused the commit its first carve asked for.
    /// Returns the region's base.
    fn assigned_and_never_carved(state: &State<Sim>, class: Class) -> *mut u8 {
        let back = state.get_or_reserve().unwrap();
        // The commit of the block's fixed part goes through; the commit of the slots does not.
        state.os().set_faults(Call::Commit, Faults::once_after(1));
        assert!(back.alloc(class, 1).is_none());
        state.os().set_faults(Call::Commit, Faults::NONE);
        let region = back.space().plan().region(0).unwrap();
        assert_eq!(state.table.get(region), RegionTag::Class(class));
        let host = back.host(class).unwrap();
        assert_eq!(
            (host.grow(region).load(), host.hints(region).load()),
            (0, 0)
        );
        back.space().region(region).as_ptr()
    }

    /// All zero is an empty, ungrown region on every path that does not abort. Natively the
    /// simulator's memory is inaccessible until committed, and the region's leaf bitmap is
    /// not, so a path that looked at it would kill the test. Under Miri the memory is not
    /// guarded, so these tests would show nothing there.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_region_with_a_zero_header_is_empty_on_every_path() {
        for class in [small(8).class(), small(1024).class(), large(64 << 10)] {
            let state = state::<Standard>();
            let base = assigned_and_never_carved(&state, class);
            let back = state.get_or_reserve().unwrap();
            assert_eq!(back.lookup(base), Some(Kind::Slot(class)));
            assert_eq!(back.trim(), 0);
            state.os().advance_clock(DECAY_TICKS);
            back.heartbeat();
            assert_eq!(state.os().calls(Call::Purge), 0);
            let mut seen = Seen::default();
            back.inspect(&mut seen);
            assert_eq!(seen.regions, [(class, 0, 0)]);
            assert_eq!(seen.free, 0);
            // And the region grows from there as if nothing had happened.
            let block = back.alloc(class, 1).unwrap();
            assert_eq!(block.ptr.as_ptr(), base);
            assert!(block.zeroed);
            back.free(block.ptr, class);
            assert_eq!(back.alloc(class, 1).unwrap().ptr.as_ptr(), base);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    #[should_panic(expected = "never handed out")]
    fn a_launder_into_a_region_with_a_zero_header_aborts() {
        let state = state::<Standard>();
        let class = small(64);
        let base = assigned_and_never_carved(&state, class.class());
        let back = state.get_or_reserve().unwrap();
        // SAFETY: the pointers are bad ones, passed to see the abort.
        unsafe { back.launder(class, &[base, base.wrapping_add(64)]) };
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    #[should_panic(expected = "never handed out")]
    fn a_large_free_into_a_region_with_a_zero_header_aborts() {
        let state = state::<Standard>();
        let base = assigned_and_never_carved(&state, large(64 << 10));
        let back = state.get_or_reserve().unwrap();
        back.free_by_address(NonNull::new(base).unwrap());
    }
}
