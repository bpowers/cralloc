//! Evidence for the adversarial review of the growth headroom rule (`e486bc4..769974a`): a
//! large block that outgrows its slot moves into one with sixteen times its room, falling back
//! to four times and then to its size.
//!
//! The simulator's native shape has regions of a megabyte, so at sixteen times every slot with
//! headroom is a class above half a region and sits alone in its region: the parent module's
//! random runs never put two such slots side by side. [`Roomy`] has regions of 32 MiB, which
//! hold several slots of sixteen times any block the runs here make, as the design's regions
//! do. The runs check the parent's properties on it (a clean bit is true, a zero bit is true,
//! the count is the free slots' pages that are not clean, and it returns to zero), and one
//! more: every page of a slot past what its tenants were given is clean, so the room nobody
//! wrote is in no count. They run also with commits failing, so that growth takes its
//! fallbacks.

use super::*;
use crate::classes::MagazineSize;
use crate::space::Geometry;

/// 4 KiB pages, 64 KiB huge pages, 123 object regions of 32 MiB: 4 GiB of address space that
/// the host commits only as it is touched.
enum Roomy {}

impl Shape for Roomy {
    const GEOMETRY: Geometry = Geometry::new(12, 16, 25, 17, 128, 1, MagazineSize::MAX);
}

/// The most a block of the runs asks for. Sixteen times it is a class a region holds three of.
const TOP: usize = 600 << 10;

/// The most blocks a run holds at once. A census reads every carved large slot, and a class
/// carves as many slots as it held blocks at once.
const MOST: usize = 8;

/// A large size, more often a small one: the smaller the block, the more slots with headroom
/// share a region.
fn roomy_size(rng: &mut Rng) -> usize {
    MAX_SMALL_SIZE
        + 1
        + match rng.below(4) {
            0 => rng.below(32 << 10),
            1 => rng.below(200 << 10),
            _ => rng.below(TOP - MAX_SMALL_SIZE),
        }
}

/// The parent's `c_realloc` for any shape, and for a heap that may have no memory: `None`
/// then, with the old block as it was.
fn c_realloc_in<S: Shape>(sim: &SimHeap<S>, old: NonNull<u8>, size: usize) -> Option<NonNull<u8>> {
    let heap = sim.heap();
    let Some(Found::Large { size: used }) = heap.find(old.as_ptr()) else {
        panic!("not a large block");
    };
    // SAFETY: the test holds the block and uses none of it during the call.
    if unsafe { heap.resize_in_place_by_address(old, used, size) } {
        return Some(old);
    }
    let kept = heap.usable_size(old.as_ptr()).unwrap();
    let new = if size > kept {
        heap.alloc_large_grown(size)
    } else {
        heap.alloc_large_aligned(size, C_ALIGN, false)
    }?;
    // SAFETY: both are live blocks the test holds, `old` given up.
    unsafe {
        heap.back()
            .unwrap()
            .move_large(old, new, kept.min(size), false)
    };
    Some(new)
}

/// What a run met, so that a pass is known to have exercised the rule under review.
#[derive(Default, Debug, Clone, Copy)]
struct Seen {
    met: Met,
    /// Growths that moved into a slot at least twice the size asked for.
    roomy: usize,
    /// Growths that moved into a slot under twice the size asked for: the last fallback.
    bare: usize,
    /// Growths that moved although the OS refused a commit during the call: a fallback taken.
    fell_back: usize,
    /// Calls that found no memory.
    refused: usize,
    /// Steps at which a region held two or more carved slots of a class of a megabyte or more.
    side_by_side: usize,
}

/// Counts a growth that moved to `ptr` for `size` bytes by the room its slot has, and by
/// whether a commit was refused on the way: `failing` is the commit failures still planned
/// when the call began.
fn note_growth<S: Shape>(
    back: &Back<'_, Sim<S>>,
    seen: &mut Seen,
    ptr: NonNull<u8>,
    size: usize,
    failing: u32,
) {
    let Some(Kind::Slot(class)) = back.lookup(ptr.as_ptr()) else {
        return;
    };
    if !class.is_small() {
        seen.fell_back += usize::from(back.state.os().faults(Call::Commit).fail < failing);
        if class.size() >= 2 * size {
            seen.roomy += 1;
        } else {
            seen.bare += 1;
        }
    }
}

/// Whether some region has two carved slots of a class of a megabyte or more: slots that only
/// a block with headroom takes here, since no block asks for more than [`TOP`].
fn side_by_side<S: Shape>(back: &Back<'_, Sim<S>>) -> bool {
    back.space().plan().regions().any(|region| {
        let RegionTag::Class(class) = back.table().get(region) else {
            return false;
        };
        class.size() >= 1 << 20
            && back
                .host(class)
                .is_some_and(|host| Grow::unpack(host.grow(region).load()).carved >= 2)
    })
}

/// For each large slot, by region and slot: the page of the region past the last that any
/// tenant since the slot was last seen free and wholly clean may have written.
type Reach = BTreeMap<(usize, u32), u32>;

/// Checks that the room nobody was given is clean. Brings `reach` up to the blocks in `live`,
/// which are all the heap's blocks, then looks at every carved large slot, held or free: no
/// page at or past its reach may have its clean bit clear, since only a claim and a widening
/// of a held block clear clean bits, and only under what the block may write. With the count
/// equal to the free slots' pages that are not clean, this is what keeps untouched room out of
/// the count. Call it after every step, so that every tenant is seen.
fn room_is_clean<S: Shape>(back: &Back<'_, Sim<S>>, live: &[Owned], reach: &mut Reach) {
    for owned in live {
        let Some(Kind::Slot(class)) = back.lookup(owned.ptr.as_ptr()) else {
            continue;
        };
        if class.is_small() {
            continue;
        }
        let layout = *back.host(class).unwrap().layout();
        let location = back.space().locate(owned.ptr.as_ptr()).unwrap();
        let (slot, _) = layout.block_at(location.offset as usize).unwrap();
        let usable = back.usable_size(owned.ptr.as_ptr()).unwrap();
        // A page more, because the back end reckons a block that is not staggered from its
        // slot's stagger; and to a huge page, because advice faults whole ones.
        let end = (location.offset as usize + usable).div_ceil(layout.page_size()) as u32 + 1;
        let most = reach.entry((location.region.index(), slot)).or_insert(0);
        *most = (*most).max(layout.huge_ceil(end));
    }
    for region in back.space().plan().regions() {
        let RegionTag::Class(class) = back.table().get(region) else {
            continue;
        };
        if class.is_small() {
            continue;
        }
        let host = back.host(class).unwrap();
        let layout = *host.layout();
        let carved = Grow::unpack(host.grow(region).load()).carved;
        for slot in 0..carved.min(layout.slots()) {
            let unit = layout.slot_unit(slot).unwrap();
            let clean = |p: u32| {
                host.meta(region, layout.clean_word(p / BITS)).load() >> (p % BITS) & 1 != 0
            };
            let key = (region.index(), slot);
            match (unit.start..unit.end).rev().find(|&p| !clean(p)) {
                Some(last) => {
                    let reached = reach.get(&key).copied().unwrap_or(0);
                    assert!(
                        last < reached,
                        "page {} of a slot of {} bytes is not clean, and no tenant reached past \
                         page {}",
                        last - unit.start,
                        class.size(),
                        reached.saturating_sub(unit.start)
                    );
                }
                None => {
                    let (word, bit) = (slot / BITS, slot % BITS);
                    if tree::word(&host, Node::leaf(region, word)).load() >> bit & 1 != 0 {
                        reach.remove(&key);
                    }
                }
            }
        }
    }
}

/// The parent's `fill_all` on [`Roomy`]: a program that fills all it is told it may use, with
/// blocks allocated plain, zeroed and aligned through the C and Rust calls, grown with
/// headroom, resized in place, moved and freed, and collections and the clocked pass between.
/// With `faults`, commits, purges and advice fail at random, so a call may find no memory;
/// the block it was resizing must then be as it was. After every step the clean bits are true,
/// no zero bit stands over data, the count is the free slots' pages that are not clean, and
/// memory promised zero is zero, and the room past what a slot's tenants were given is clean;
/// at the end nothing is resident and the count is zero.
fn fill_roomy(seed: u64, steps: usize, faults: bool) -> Seen {
    let sim = SimHeap::<Roomy>::new();
    let os = sim.os();
    let heap = sim.heap();
    let page = Roomy::GEOMETRY.page_size();
    let mut rng = Rng(seed | 1);
    let mut live: Vec<Owned> = Vec::new();
    let (mut fill, mut seen) = (0u8, Seen::default());
    let mut reach = Reach::new();
    let write = |ptr: NonNull<u8>, len: usize, fill: u8| paint(ptr.as_ptr(), len, fill);
    // Start the heap before anything fails.
    let first = heap
        .alloc_large_aligned(roomy_size(&mut rng), C_ALIGN, false)
        .unwrap();
    live.push(Owned {
        ptr: first,
        layout: None,
        wrote: 0,
        fill,
    });
    // Seen before the first step, which may free it.
    room_is_clean(&heap.back().unwrap(), &live, &mut reach);
    for at in 0..steps {
        if faults && rng.below(3) == 0 {
            let call = [Call::Commit, Call::Commit, Call::Purge, Call::AdviseHuge][rng.below(4)];
            let plan = match rng.below(3) {
                0 => Faults::NONE,
                1 => Faults::once_after(rng.below(3) as u32),
                _ => Faults {
                    pass: rng.below(3) as u32,
                    fail: 1 + rng.below(3) as u32,
                    partial: rng.below(2) == 0,
                },
            };
            os.set_faults(call, plan);
        }
        fill = fill.wrapping_add(1).max(1);
        let pick = |rng: &mut Rng, live: &Vec<Owned>, c: bool| {
            let of: Vec<usize> = (0..live.len())
                .filter(|&i| live[i].layout.is_none() == c)
                .collect();
            (!of.is_empty()).then(|| of[rng.below(of.len())])
        };
        match rng.below(16) {
            // C: malloc, calloc, aligned_alloc.
            0..=2 if live.len() < MOST => {
                let (size, align, zero) = match rng.below(4) {
                    0 => (roomy_size(&mut rng), C_ALIGN, true),
                    1 => {
                        let size = [0, 1, 100, 5000, 40 << 10, 200 << 10][rng.below(6)];
                        (size, (64 << 10) << rng.below(3), rng.below(4) == 0)
                    }
                    _ => (roomy_size(&mut rng), C_ALIGN, false),
                };
                match heap.alloc_large_aligned(size, align, zero) {
                    Some(ptr) => {
                        assert_eq!(ptr.addr().get() % align, 0);
                        let usable = heap.usable_size(ptr.as_ptr()).unwrap();
                        assert!(usable >= size);
                        if zero {
                            assert!(
                                all_are(ptr.as_ptr(), usable, 0),
                                "seed {seed:#x} step {at}: a zeroed block of {size} bytes is \
                                 not zero"
                            );
                        }
                        write(ptr, usable, fill);
                        live.push(Owned {
                            ptr,
                            layout: None,
                            wrote: usable,
                            fill,
                        });
                    }
                    None => {
                        assert!(faults, "seed {seed:#x} step {at}: no block of {size} bytes");
                        seen.refused += 1;
                    }
                }
            }
            // C: realloc, more often up than down.
            3..=6 => {
                if let Some(i) = pick(&mut rng, &live, true) {
                    let old = &live[i];
                    let size = match rng.below(3) {
                        0 => roomy_size(&mut rng),
                        _ => (old.wrote + 1 + rng.below(old.wrote.max(1)))
                            .clamp(MAX_SMALL_SIZE + 1, TOP),
                    };
                    let failing = os.faults(Call::Commit).fail;
                    match c_realloc_in(&sim, old.ptr, size) {
                        Some(ptr) => {
                            let usable = heap.usable_size(ptr.as_ptr()).unwrap();
                            assert!(usable >= size);
                            assert!(
                                all_are(ptr.as_ptr(), old.wrote.min(size), old.fill),
                                "seed {seed:#x} step {at}: realloc to {size} lost bytes"
                            );
                            if ptr != old.ptr && size > old.wrote {
                                let back = heap.back().unwrap();
                                note_growth(&back, &mut seen, ptr, size, failing);
                            }
                            write(ptr, usable, fill);
                            live[i] = Owned {
                                ptr,
                                layout: None,
                                wrote: usable,
                                fill,
                            };
                        }
                        None => {
                            assert!(faults, "seed {seed:#x} step {at}: no realloc to {size}");
                            assert!(all_are(old.ptr.as_ptr(), old.wrote, old.fill));
                            seen.refused += 1;
                        }
                    }
                }
            }
            // C: free.
            7 => {
                if let Some(i) = pick(&mut rng, &live, true) {
                    let old = live.swap_remove(i);
                    assert!(all_are(old.ptr.as_ptr(), old.wrote, old.fill));
                    // SAFETY: the test's block, freed once.
                    unsafe { heap.free_large(old.ptr) };
                }
            }
            // Rust: alloc and alloc_zeroed, a few small classes among them.
            8 | 9 if live.len() < MOST => {
                let size = match rng.below(5) {
                    0 => [100, 5000, 30_000][rng.below(3)],
                    _ => roomy_size(&mut rng),
                };
                // Up to four huge pages, so that some blocks that grow are aligned beyond one.
                let align = [8, 16, page, 64 << 10, 256 << 10][rng.below(5)];
                let layout = Layout::from_size_align(size, align).unwrap();
                let zero = rng.below(3) == 0;
                // SAFETY: the layout's size is not zero.
                let ptr = unsafe {
                    if zero {
                        GlobalAlloc::alloc_zeroed(&sim, layout)
                    } else {
                        GlobalAlloc::alloc(&sim, layout)
                    }
                };
                match NonNull::new(ptr) {
                    Some(ptr) => {
                        if zero {
                            assert!(
                                all_are(ptr.as_ptr(), size, 0),
                                "seed {seed:#x} step {at}: alloc_zeroed of {size} bytes is not \
                                 zero"
                            );
                        }
                        write(ptr, size, fill);
                        live.push(Owned {
                            ptr,
                            layout: Some(layout),
                            wrote: size,
                            fill,
                        });
                    }
                    None => {
                        assert!(faults, "seed {seed:#x} step {at}: no block of {size} bytes");
                        seen.refused += 1;
                    }
                }
            }
            // Rust: realloc, and a zeroed grow.
            10..=12 => {
                if let Some(i) = pick(&mut rng, &live, false) {
                    let old = &live[i];
                    let layout = old.layout.unwrap();
                    let grow_zeroed = rng.below(4) == 0;
                    let size = match rng.below(4) {
                        _ if grow_zeroed => layout.size() + 1 + rng.below(200 << 10),
                        0 => [100, 5000, 30_000][rng.below(3)],
                        1 => roomy_size(&mut rng),
                        _ => (layout.size() + 1 + rng.below(layout.size())).min(TOP),
                    };
                    let new = Layout::from_size_align(size, layout.align()).unwrap();
                    let kept = old.wrote.min(layout.size()).min(size);
                    let failing = os.faults(Call::Commit).fail;
                    let got = if grow_zeroed {
                        let before = sim.usable_size(old.ptr.as_ptr()).unwrap();
                        // SAFETY: the test's block, which `layout` fits; `new` is larger.
                        unsafe { sim.grow_zeroed_for_test(old.ptr.as_ptr(), layout, new) }.map(
                            |(ptr, len)| {
                                assert!(len >= size);
                                assert_eq!(
                                    sim.usable_size(ptr),
                                    Some(len),
                                    "seed {seed:#x} step {at}: a grant of {len} bytes for {size}"
                                );
                                // Past the old block, which ended at its usable size if it
                                // stayed and at its layout's if it moved.
                                let from = before.max(layout.size()).min(len);
                                assert!(
                                    all_are(ptr.wrapping_add(from), len - from, 0),
                                    "seed {seed:#x} step {at}: a zeroed grow to {size} is not \
                                     zero"
                                );
                                (ptr, len)
                            },
                        )
                    } else {
                        // SAFETY: the test's block, which `layout` fits; `size` is not zero.
                        let ptr =
                            unsafe { GlobalAlloc::realloc(&sim, old.ptr.as_ptr(), layout, size) };
                        (!ptr.is_null()).then_some((ptr, size))
                    };
                    match got {
                        Some((ptr, wrote)) => {
                            let ptr = NonNull::new(ptr).unwrap();
                            assert!(
                                all_are(ptr.as_ptr(), kept, old.fill),
                                "seed {seed:#x} step {at}: a resize to {size} lost bytes"
                            );
                            // The usable size is the size asked for, to a page, whatever room
                            // the slot has.
                            let usable = sim.usable_size(ptr.as_ptr()).unwrap();
                            assert!(
                                small_or(size, usable) || usable < size + page,
                                "seed {seed:#x} step {at}: {usable} usable for {size}"
                            );
                            if ptr != old.ptr && !grow_zeroed && size > layout.size() {
                                let back = heap.back().unwrap();
                                note_growth(&back, &mut seen, ptr, size, failing);
                            }
                            write(ptr, wrote, fill);
                            // Any size from the one asked for to the length granted fits.
                            let fits = Layout::from_size_align(wrote, layout.align()).unwrap();
                            live[i] = Owned {
                                ptr,
                                layout: Some(fits),
                                wrote,
                                fill,
                            };
                        }
                        None => {
                            assert!(faults, "seed {seed:#x} step {at}: no resize to {size}");
                            assert!(all_are(old.ptr.as_ptr(), old.wrote, old.fill));
                            seen.refused += 1;
                        }
                    }
                }
            }
            // Rust: dealloc.
            13 => {
                if let Some(i) = pick(&mut rng, &live, false) {
                    let old = live.swap_remove(i);
                    assert!(all_are(old.ptr.as_ptr(), old.wrote, old.fill));
                    // SAFETY: the test's block, which the layout fits, freed once.
                    unsafe { GlobalAlloc::dealloc(&sim, old.ptr.as_ptr(), old.layout.unwrap()) };
                }
            }
            14 => {
                os.advance_clock(DECAY_TICKS);
                sim.maintain();
            }
            _ if rng.below(4) == 0 => {
                sim.collect();
            }
            _ => {}
        }
        let back = heap.back().unwrap();
        let census = census(&back);
        assert_eq!(census.wrong_zero, 0, "seed {seed:#x} step {at}");
        assert_eq!(
            (census.hidden, census.hidden_held),
            (0, 0),
            "seed {seed:#x} step {at}: a clean bit over a written page ({census:?})"
        );
        for owned in &live {
            let usable = heap.usable_size(owned.ptr.as_ptr()).unwrap();
            assert!(owned.wrote <= usable);
            assert_eq!(
                clean_under(&back, owned.ptr, usable),
                0,
                "seed {seed:#x} step {at}: a clean bit under a block of {usable} bytes"
            );
        }
        assert_eq!(
            back.state.retained.large.load(Relaxed),
            census.counted as isize,
            "seed {seed:#x} step {at}: the count against the clean bits ({census:?})"
        );
        room_is_clean(&back, &live, &mut reach);
        seen.met.add(&census);
        seen.side_by_side += usize::from(side_by_side(&back));
    }
    for call in Call::ALL {
        os.set_faults(call, Faults::NONE);
    }
    for owned in live.drain(..) {
        assert!(all_are(owned.ptr.as_ptr(), owned.wrote, owned.fill));
        // SAFETY: the test's blocks, each freed once.
        unsafe {
            match owned.layout {
                Some(layout) => GlobalAlloc::dealloc(&sim, owned.ptr.as_ptr(), layout),
                None => heap.free_large(owned.ptr),
            }
        }
    }
    sim.collect();
    let back = heap.back().unwrap();
    let census = census(&back);
    assert_eq!((census.resident, census.wrong_zero), (0, 0), "{census:?}");
    assert_eq!(back.state.retained.large.load(Relaxed), 0);
    seen
}

/// Whether `size` has a small class, whose usable size is the class and not `usable` to a page.
fn small_or(size: usize, usable: usize) -> bool {
    size <= MAX_SMALL_SIZE && usable <= MAX_SMALL_SIZE
}

fn roomy_runs(salt: u64, seeds: u64, steps: usize, faults: bool) {
    let mut seen = Seen::default();
    for seed in 0..seeds {
        let one = fill_roomy(salt ^ seed, steps, faults);
        seen.met.join(one.met);
        seen.roomy += one.roomy;
        seen.bare += one.bare;
        seen.fell_back += one.fell_back;
        seen.refused += one.refused;
        seen.side_by_side += one.side_by_side;
    }
    assert!(
        seen.met.zero != 0
            && seen.met.told != 0
            && seen.met.advised != 0
            && seen.roomy != 0
            && seen.side_by_side != 0,
        "the runs missed a case: {seen:?}"
    );
    // With commits failing, some growth must have been refused a slot and taken another, and
    // some call must have found no memory at all. With none failing, every growth of a large
    // block gets room: sixteen times the largest block here fits a region.
    if faults {
        assert!(
            seen.fell_back != 0 && seen.refused != 0,
            "the runs missed a fallback: {seen:?}"
        );
    } else {
        assert_eq!(seen.bare, 0, "a growth got no room: {seen:?}");
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn slots_with_headroom_side_by_side_keep_the_clean_bits_true_quickly() {
    roomy_runs(0x6a09_e667_f3bc_c908, 1, 250, false);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn slots_with_headroom_side_by_side_keep_the_clean_bits_true_fully() {
    roomy_runs(0x6a09_e667_f3bc_c908, 8 / DEBUG_LESS, 1200, false);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn growth_that_falls_back_when_commits_fail_keeps_the_clean_bits_true() {
    roomy_runs(0xbb67_ae85_84ca_a73b, 8 / DEBUG_LESS, 1200, true);
}
