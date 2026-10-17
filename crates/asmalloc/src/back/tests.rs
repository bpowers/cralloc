//! The back end over the simulated OS: unit tests, fault injection, a shadow-model property test
//! and a threaded stress test.

use super::policy::retained_cap;
use super::{Back, Block, Kind, State};
use crate::classes::{Class, SmallClass};
use crate::os::sim::{Call, Faults, Native, Shape, SimOs, Standard};
use crate::os::{HugePages, Os};
use crate::space::RegionTag;
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
    // 8 KiB slots: enough for two full regions and a few in a third.
    let class = small(8 << 10);
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
    let before = os.resident_bytes();
    assert!(before >= n * class.size());
    for b in &blocks {
        back.free(b.ptr, class);
    }
    assert!(back.retained() <= cap, "{} > {cap}", back.retained());
    let returned = before - os.resident_bytes();
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
    assert!(
        os.resident_bytes() < before / 8,
        "trim leaves {} resident",
        os.resident_bytes()
    );
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
    assert!(
        !state
            .os()
            .is_purged(core::ptr::NonNull::new(low).unwrap(), 1024)
    );
}

#[test]
fn heartbeats_keep_purging_while_over_the_cap() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let cap = retained_cap(Standard::GEOMETRY);
    let class = large(40 << 10);
    // Large slots, one to a region; free them one at a time so that the frees after the one
    // that crosses the cap do not purge, then let heartbeats finish.
    let blocks: Vec<Block> = (0..3 * cap / class.size())
        .map(|_| back.alloc(class, class.size()).unwrap())
        .collect();
    for block in &blocks {
        paint(block.ptr.as_ptr(), class.size(), 1);
    }
    for block in &blocks {
        back.free(block.ptr, class);
    }
    for _ in 0..1000 {
        back.heartbeat();
    }
    assert!(back.retained() <= cap);
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
