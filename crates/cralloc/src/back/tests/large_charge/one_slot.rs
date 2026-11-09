//! One large slot put through every history its bits have to survive, on each kernel's pages.
//!
//! The parent's random runs spread their blocks over many classes, and only now and then put a
//! block where a page stagger matters: at the start of a slot a few pages of stagger long, shrunk
//! below those pages and grown back over them. So one slot here takes every step: a tenant at its
//! start or at its stagger, sizes on either side of the stagger's pages and of the slot's end,
//! writes up to all the tenant may use, moves, frees, and while it is free purges in arbitrary
//! pieces, the hold and trims. After every step no page marked clean holds data, none stands under
//! what a held block may use, and a free slot whose zero bit is set reads as zero. A claim told
//! zero reads the whole slot.
//!
//! The table's slots run from one page of stagger to the most a slot can have, and all are under a
//! huge page. Those of [`HUGE_PAGES`] and [`LARGE_HUGE_PAGES`] are over one, so their blocks get
//! advised, and on the second a piece of a move is more than the usual two megabytes.

use super::*;
use crate::back::layout::Unit;
use crate::back::policy::FINE_TICKS;
use crate::back::region::{self, Purge};
use crate::os::sim::{Kernel4k, Kernel16k, Kernel64k, room_for};

/// Slots, by the pages of their class and their index in its region. A slot staggers by its index
/// modulo an eighth of its pages, at most 32.
const SLOTS: [(usize, usize); 9] = [
    (16, 1),
    (16, 7),
    (32, 3),
    (32, 31),
    (64, 5),
    (64, 31),
    (128, 15),
    (256, 7),
    (256, 31),
];

/// The slots of the form in the plain gate: a few pages of stagger, and the most.
const FEW_SLOTS: [(usize, usize); 2] = [(64, 5), (256, 31)];

/// Slots of a class above a huge page, on the kernels where one is used and such a class is not
/// too dear to read at every step: 4 and 8 MiB on pages of 4 KiB.
const HUGE_PAGES: [(usize, usize); 2] = [(1024, 3), (2048, 5)];

/// A slot of 64 MiB on pages of 16 KiB, whose huge page of 32 MiB is more than a piece of a move.
const LARGE_HUGE_PAGES: [(usize, usize); 1] = [(4096, 3)];

/// What a history met, so that a pass is known to have exercised what these tests are for.
#[derive(Default, Clone, Copy, Debug)]
struct Seen {
    /// Claims told the slot reads as zero.
    zeroes: usize,
    /// Steps at which the slot was advised for huge pages.
    advised: usize,
}

/// Writes a byte in each page of what the block may use, and its last, as a program that asks for
/// its usable size may.
fn touch<O: Os>(back: &Back<'_, O>, block: NonNull<u8>, page: usize, byte: u8) {
    let usable = back.usable_size(block.as_ptr()).unwrap();
    let start = block.addr().get();
    let mut at = 0;
    while at < usable {
        // SAFETY: inside what the block may use.
        unsafe { block.as_ptr().add(at).write(byte) };
        at = (start + at + 1).next_multiple_of(page) - start;
    }
    if usable != 0 {
        // SAFETY: as above.
        unsafe { block.as_ptr().add(usable - 1).write(byte) };
    }
}

/// A size for a block with `room` bytes of room: at the edges of a page and of a slot's `stagger`,
/// or a large part of the room.
fn a_size(rng: &mut Rng, page: usize, stagger: usize, room: usize) -> usize {
    let top = (stagger / page).max(1) * page;
    let sizes = [
        1,
        page - 1,
        page,
        page + 1,
        2 * page,
        top - 1,
        top,
        top + 1,
        top + page,
        2 * top + page,
        room / 4,
        room / 2,
        room - page,
        room - 1,
        room,
    ];
    sizes[rng.below(sizes.len())].clamp(1, room)
}

/// `steps` steps of a random history of slot `index` of the class of `pages` pages.
fn history<S: Shape>(seed: u64, steps: usize, faults: bool, pages: usize, index: usize) -> Seen {
    let state = state::<S>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    state.hold.unfloored.store(true, Relaxed);
    let page = S::GEOMETRY.page_size();
    let class = large_of(pages * page);
    assert_eq!(class.size(), pages * page);
    let wider = large_of(2 * pages * page);
    let host = back.host(class).unwrap();
    let layout = *host.layout();
    // The slots below stay held, so that each tenant here takes this one.
    for _ in 0..index {
        back.alloc_staggered(class, page).unwrap();
    }
    let first = back.alloc(class, page).unwrap().ptr;
    let location = back.space().locate(first.as_ptr()).unwrap();
    let region = location.region;
    let (slot, _) = layout.block_at(location.offset as usize).unwrap();
    assert_eq!(slot as usize, index);
    back.free(first, class);
    let unit = layout.slot_unit(slot).unwrap();
    let stagger = layout.stagger(slot);
    let base = back.space().slot(region, class, slot);
    let mut rng = Rng(seed | 1);
    let mut held: Option<NonNull<u8>> = None;
    let (mut byte, mut seen) = (0u8, Seen::default());
    for at in 0..steps {
        byte = byte.wrapping_add(1).max(1);
        if faults && rng.below(8) == 0 {
            let call = [Call::Purge, Call::AdviseHuge, Call::Resident][rng.below(3)];
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
        match held {
            None => match rng.below(10) {
                0..=3 => {
                    let size = a_size(&mut rng, page, stagger, class.size());
                    let block = if rng.below(2) == 0 {
                        back.alloc_staggered(class, size)
                    } else {
                        back.alloc(class, size)
                    }
                    .unwrap();
                    let offset = back.space().locate(block.ptr.as_ptr()).unwrap().offset;
                    assert_eq!(layout.block_at(offset as usize).unwrap().0, slot);
                    if block.zeroed {
                        seen.zeroes += 1;
                        assert!(
                            all_are(base.as_ptr(), class.size(), 0),
                            "seed {seed:#x} step {at}: told zero, and it is not"
                        );
                        if rng.below(4) == 0 {
                            back.mark_unwritten(block.ptr);
                        }
                    }
                    if rng.below(3) != 0 {
                        touch(&back, block.ptr, page, byte);
                    }
                    held = Some(block.ptr);
                }
                4..=6 => {
                    let piece = if rng.below(3) == 0 {
                        unit
                    } else {
                        let a = unit.start + rng.below(pages) as u32;
                        let b = unit.start + rng.below(pages) as u32;
                        Unit {
                            start: a.min(b),
                            end: a.max(b) + 1,
                        }
                    };
                    if let (Purge::Done { returned, outgrown }, _) =
                        region::purge_unit(&host, region, piece)
                    {
                        back.forget_outgrown(outgrown);
                        back.unretain(class, returned);
                    }
                }
                7 | 8 => {
                    for _ in 0..1 + rng.below(16) {
                        os.advance_clock(FINE_TICKS);
                        back.heartbeat();
                    }
                }
                _ => {
                    back.trim();
                }
            },
            Some(block) => match rng.below(10) {
                0..=2 => touch(&back, block, page, byte),
                3..=6 => {
                    let room = back.room(block.as_ptr()).unwrap();
                    // A doubling now and then, which is what gets a block advised.
                    let size = if rng.below(4) == 0 {
                        (2 * back.size(block.as_ptr()).unwrap()).clamp(1, room)
                    } else {
                        a_size(&mut rng, page, stagger, room)
                    };
                    if rng.below(5) == 0 {
                        back.resize(block, size);
                    } else {
                        // SAFETY: the test holds the block and uses none of it during the call.
                        unsafe { back.resize_in_place(block, size) };
                    }
                    if rng.below(2) == 0 {
                        touch(&back, block, page, byte);
                    }
                }
                7 | 8 => {
                    if rng.below(2) == 0 {
                        back.free(block, class);
                    } else {
                        back.free_by_address(block);
                    }
                    held = None;
                }
                _ => {
                    let len = back.size(block.as_ptr()).unwrap();
                    let to = back.alloc(wider, len.max(1)).unwrap();
                    let why = if rng.below(2) == 0 {
                        crate::back::Move::Outgrown
                    } else {
                        crate::back::Move::Other
                    };
                    // SAFETY: both are live blocks the test holds, `block` given up.
                    unsafe { back.move_large(block, to.ptr, len, to.zeroed, why) };
                    back.free(to.ptr, wider);
                    held = None;
                }
            },
        }
        seen.advised += usize::from(region::recorded(&host, region, slot).is_some_and(|r| r.huge));
        let (word, bit) = (slot / BITS, slot % BITS);
        let free = tree::word(&host, Node::leaf(region, word)).load() >> bit & 1 != 0;
        let zero = host.meta(region, layout.zero_word(word).unwrap()).load() >> bit & 1 != 0;
        for p in unit.start..unit.end {
            let clean = host.meta(region, layout.clean_word(p / BITS)).load() >> (p % BITS) & 1;
            if clean != 0 || (zero && free) {
                let at_page = base.as_ptr().wrapping_add((p - unit.start) as usize * page);
                assert!(
                    !holds_data(at_page, page),
                    "seed {seed:#x} step {at}: page {} of {pages} holds data, clean {clean}, zero \
                     {zero}, free {free}, stagger {stagger:#x}, held {held:?}",
                    p - unit.start
                );
            }
        }
        if let Some(block) = held {
            let usable = back.usable_size(block.as_ptr()).unwrap();
            assert_eq!(
                clean_under(&back, block, usable),
                0,
                "seed {seed:#x} step {at}: a clean bit under a block of {usable} bytes, stagger \
                 {stagger:#x}"
            );
        }
    }
    for call in Call::ALL {
        os.set_faults(call, Faults::NONE);
    }
    seen
}

/// Histories of each of `slots` on `S`, which must between them have met a claim told zero. A
/// process with no room for the shape passes over it, as the kernels' tests do.
fn histories<S: Shape>(slots: &[(usize, usize)], seeds: u64, steps: usize, faults: bool) -> Seen {
    let mut seen = Seen::default();
    if !room_for::<S>() {
        return seen;
    }
    for &(pages, index) in slots {
        for seed in 0..seeds {
            let salt = 0x9e37_79b9 ^ (seed << 20) ^ (pages as u64) << 8 ^ index as u64;
            let one = history::<S>(salt, steps, faults, pages, index);
            seen.zeroes += one.zeroes;
            seen.advised += one.advised;
        }
    }
    assert!(seen.zeroes != 0, "the histories missed a case: {seen:?}");
    seen
}

/// A few hundred steps of a slot, which is about what a bug in its bits takes to show.
fn quickly<S: Shape>() {
    histories::<S>(&FEW_SLOTS, 1, 400, false);
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_slots_bits_stay_true_through_its_histories_quickly_at_4_kib() {
    quickly::<Kernel4k>();
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_slots_bits_stay_true_through_its_histories_quickly_at_16_kib() {
    quickly::<Kernel16k>();
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_slots_bits_stay_true_through_its_histories_quickly_at_64_kib() {
    quickly::<Kernel64k>();
}

/// The whole table, with calls that succeed and with calls that fail.
fn fully<S: Shape>() {
    let steps = 4000 / DEBUG_LESS as usize;
    histories::<S>(&SLOTS, 2, steps, false);
    histories::<S>(&SLOTS, 2, steps, true);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_slots_bits_stay_true_through_its_histories_fully_at_4_kib() {
    fully::<Kernel4k>();
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_slots_bits_stay_true_through_its_histories_fully_at_16_kib() {
    fully::<Kernel16k>();
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_slots_bits_stay_true_through_its_histories_fully_at_64_kib() {
    fully::<Kernel64k>();
}

/// Slots that are advised, whose pages are the dearest to read, so fewer steps.
fn under_advice<S: Shape>(slots: &[(usize, usize)], steps: usize) {
    for faults in [false, true] {
        let seen = histories::<S>(slots, 1, steps, faults);
        assert!(
            seen.advised != 0 || !room_for::<S>(),
            "no history was advised: {seen:?}"
        );
    }
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_slots_bits_stay_true_under_advice_at_2_mib() {
    under_advice::<Kernel4k>(&HUGE_PAGES, 2000 / DEBUG_LESS as usize);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_slots_bits_stay_true_under_advice_at_32_mib() {
    under_advice::<Kernel16k>(&LARGE_HUGE_PAGES, 2000 / DEBUG_LESS as usize);
}
