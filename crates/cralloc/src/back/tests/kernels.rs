//! The heap's rules on simulated kernels at their real size (`os::sim::Kernel4k` and the rest): a
//! rule about memory acts at the bytes it says on every kernel, a huge page is used where it is
//! worth having, and a block's stagger costs a free slot nothing it did not write.

use super::super::policy::{
    FINE_TICKS, HOLD_STEPS, allowance_floor, hold_beat, stagger_pages, stagger_room,
};
use super::super::tree::Host as _;
use super::super::word::Word as _;
use super::*;
use crate::back::Move;
use crate::os::sim::{Kernel4k, Kernel16k, Kernel64k, room_for};
use crate::space::Geometry;

const KIB: usize = 1 << 10;
const MIB: usize = 1 << 20;

/// The test `$body::<S>()` once for each kernel.
macro_rules! on_every_kernel {
    ($(#[$attr:meta])* $name:ident, $body:ident) => {
        mod $name {
            use super::*;
            $(#[$attr])*
            #[test]
            #[cfg_attr(miri, ignore)]
            fn with_4_kib_pages() {
                $body::<Kernel4k>();
            }
            $(#[$attr])*
            #[test]
            #[cfg_attr(miri, ignore)]
            fn with_16_kib_pages() {
                $body::<Kernel16k>();
            }
            $(#[$attr])*
            #[test]
            #[cfg_attr(miri, ignore)]
            fn with_64_kib_pages() {
                $body::<Kernel64k>();
            }
        }
    };
}

/// The slot a block starts in, and how far into it, by its address.
fn place_of<O: Os>(back: &Back<'_, O>, class: Class, ptr: NonNull<u8>) -> (usize, usize) {
    let offset = back.space().locate(ptr.as_ptr()).unwrap().offset as usize;
    let host = back.host(class).unwrap();
    let (slot, into) = host.layout().block_at(offset).unwrap();
    (slot as usize, into)
}

/// Free three caps' worth of one class, then grow another: over the cap each step of growth pays,
/// and retained memory comes down to 64 MiB and stays there, on every kernel.
fn the_cap_acts_at_its_bytes<S: Shape>() {
    if !room_for::<S>() {
        return;
    }
    let state = floored_state::<S>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let plan = back.space().plan().geometry();
    assert_eq!(retained_cap(plan), 64 * MIB);
    assert_eq!(growth_step(plan), 2 * MIB);
    let (freed, grown) = (small(16 * KIB), small(KIB));
    let n = 3 * retained_cap(plan) / freed.size();
    let slots = fill(&back, freed, n);
    assert_eq!(slots.len(), n);
    // SAFETY: the test holds these slots and uses none of them.
    unsafe { back.launder(freed, &slots) };
    assert_eq!(back.retained(), 192 * MIB);
    let mut live = Vec::new();
    let mut carved = 0;
    while back.retained() > 64 * MIB {
        live.extend(fill(&back, grown, 2 * MIB / KIB));
        carved += 2 * MIB;
        // A payment returns a step, and the heartbeats after it the rest.
        for _ in 0..8 {
            back.heartbeat();
        }
        assert!(carved <= 192 * MIB, "{} still retained", back.retained());
    }
    assert!(carved >= 128 * MIB - 8 * MIB, "came down after {carved}");
    let purges = os.calls(Call::Purge);
    live.extend(fill(&back, grown, 8 * MIB / KIB));
    back.heartbeat();
    assert_eq!(
        os.calls(Call::Purge),
        purges,
        "under the cap, growth pays nothing"
    );
}

on_every_kernel!(the_retained_cap, the_cap_acts_at_its_bytes);

/// A quiet heap's clocked pass stops at the floor, 32 MiB on every kernel, and takes nothing more.
fn the_floor_holds_at_its_bytes<S: Shape>() {
    if !room_for::<S>() {
        return;
    }
    let state = floored_state::<S>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let floor = allowance_floor(back.space().plan().geometry());
    assert_eq!(floor, 32 * MIB);
    let class = small(16 * KIB);
    let slots = fill(&back, class, 3 * floor / class.size());
    // SAFETY: the test holds these slots and uses none of them.
    unsafe { back.launder(class, &slots) };
    forget_reuse(&back);
    os.advance_clock(DECAY_TICKS);
    back.decay_pass(&|| back.now());
    let left = back.retained();
    let unit = class.size().max(S::GEOMETRY.page_size());
    assert!(
        left <= floor && left + 4 * unit >= floor,
        "{left} left over a floor of {floor}"
    );
    let purges = os.calls(Call::Purge);
    os.advance_clock(DECAY_TICKS);
    back.decay_pass(&|| back.now());
    os.advance_clock(DECAY_TICKS);
    back.decay_pass(&|| back.now());
    assert_eq!(
        os.calls(Call::Purge),
        purges,
        "under the floor, the clock returns nothing"
    );
}

on_every_kernel!(the_allowance_floor, the_floor_holds_at_its_bytes);

/// Fills `bytes` of `class`, and returns the last slot.
fn carve_to<O: Os>(back: &Back<'_, O>, class: SmallClass, bytes: usize) -> Vec<*mut u8> {
    let slots = fill(back, class, bytes / class.size());
    assert_eq!(slots.len(), bytes / class.size());
    slots
}

/// A dense class keeps its first 8 huge pages on small pages, and is advised for huge pages past
/// them, however large they are: 16 MiB of 2 MiB pages, 256 MiB of 32 MiB. A class over 1 KiB is
/// not, whatever the page.
fn a_dense_class_gets_huge_pages_past_eight_of_them<S: Shape>() {
    if !room_for::<S>() {
        return;
    }
    let state = state::<S>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = S::GEOMETRY.huge_page_size();
    let threshold = 8 * huge;
    let dense = small(KIB);
    let slots = carve_to(&back, dense, threshold + 2 * huge);
    let advice = |p: *mut u8| os.huge_pages(NonNull::new(p).unwrap());
    assert_eq!(advice(slots[0]), Some(HugePages::Avoid));
    assert_eq!(advice(*slots.last().unwrap()), Some(HugePages::Prefer));
    let location = back.space().locate(*slots.last().unwrap()).unwrap();
    let host = back.host(dense.class()).unwrap();
    let hints = super::super::layout::Hints::unpack(host.hints(location.region).load());
    let from = hints.huge_from.unwrap() as usize * S::GEOMETRY.page_size();
    assert_eq!(from, threshold);
    // The next class up from a kilobyte never does, though it has more than four slots to a page
    // of 16 KiB, which is what the rule asks of a page unit.
    let sparse = small(KIB + 1);
    let slots = carve_to(&back, sparse, threshold + 2 * huge);
    assert_eq!(advice(*slots.last().unwrap()), Some(HugePages::Avoid));
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_dense_class_gets_huge_pages_past_eight_of_them_at_2_mib() {
    a_dense_class_gets_huge_pages_past_eight_of_them::<Kernel4k>();
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_dense_class_gets_huge_pages_past_eight_of_them_at_32_mib() {
    a_dense_class_gets_huge_pages_past_eight_of_them::<Kernel16k>();
}

/// A class's second region starts on huge pages where they are used, and on no kernel asks for
/// them where they are not: a huge page over the cap (512 MiB in regions of 1 GiB), or one that
/// does not fit a region (32 MiB in regions of 8 MiB, what a limit on address space leaves).
fn a_class_past_a_region<S: Shape>(state: &State<Sim<S>>, usable: bool) {
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let geometry = back.space().plan().geometry();
    assert_eq!(geometry.huge_pages_usable(), usable);
    let class = small(KIB);
    let region = back.space().plan().region_size();
    let slots = carve_to(&back, class, region + region / 4);
    let last = NonNull::new(*slots.last().unwrap()).unwrap();
    let advice = os.huge_pages(last);
    let want = if usable {
        HugePages::Prefer
    } else {
        HugePages::Avoid
    };
    assert_eq!(advice, Some(want));
    let location = back.space().locate(slots[0]).unwrap();
    let first = back.space().locate(last.as_ptr()).unwrap();
    assert_ne!(location.region, first.region, "two regions");
    // The rules' bytes do not move with the region.
    assert_eq!(retained_cap(geometry), 64 * MIB);
    assert_eq!(allowance_floor(geometry), 32 * MIB);
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_huge_page_over_the_cap_is_never_asked_for() {
    if !room_for::<Kernel64k>() {
        return;
    }
    let state = floored_state::<Kernel64k>();
    a_class_past_a_region(&state, false);
}

/// A reservation of `Kernel16k` with regions of 8 MiB, under its 32 MiB huge page.
fn small_regions() -> Box<State<Sim<Kernel16k>>> {
    let state = floored_state::<Kernel16k>();
    // Regions of 1 GiB down to 16 MiB are refused, as a limit on address space would.
    let faults = Faults {
        pass: 0,
        fail: 7,
        partial: false,
    };
    state.os().set_faults(Call::Reserve, faults);
    state
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_huge_page_larger_than_a_region_is_never_asked_for() {
    if !room_for::<Kernel16k>() {
        return;
    }
    let state = small_regions();
    {
        let back = state.get_or_reserve().unwrap();
        assert_eq!(back.space().plan().region_size(), 8 * MIB);
    }
    a_class_past_a_region(&state, false);
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_class_past_a_region_keeps_its_huge_pages_where_they_fit() {
    if !room_for::<Kernel4k>() {
        return;
    }
    let state = floored_state::<Kernel4k>();
    a_class_past_a_region(&state, true);
}

/// A block that grows densely gets no advice where huge pages are not used, though its size passes
/// what a plan takes for one.
#[test]
#[cfg_attr(miri, ignore)]
fn a_block_grown_where_huge_pages_are_not_used_is_never_advised() {
    if !room_for::<Kernel16k>() {
        return;
    }
    let state = small_regions();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(8 * MIB);
    assert!(back.space().plan().fits_region(class));
    let block = back.alloc(class, 4 * MIB).unwrap().ptr;
    paint(block.as_ptr(), 4 * MIB, 3);
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(block, 8 * MIB) };
    assert_eq!(os.huge_pages(block), Some(HugePages::Avoid));
    assert!(painted(block.as_ptr(), 4 * MIB, 3));
    let host = back.host(class).unwrap();
    let region = back.space().locate(block.as_ptr()).unwrap().region;
    let recorded = super::super::region::recorded(&host, region, 0).unwrap();
    assert!(!recorded.huge, "recorded as advised");
}

/// Blocks of a class whose slots hold sixteen pages or more start at both terms of the rule, and
/// everything that maps a pointer back to its slot agrees, wherever the block starts.
fn blocks_stagger_by_lines_and_pages<S: Shape>() {
    if !room_for::<S>() {
        return;
    }
    let state = state::<S>();
    let back = state.get_or_reserve().unwrap();
    let geometry = back.space().plan().geometry();
    let page = geometry.page_size();
    for pages in [16, 32, 64, 256, 1024] {
        let class = large(pages * page);
        let term = stagger_pages(geometry, class.size()) as usize;
        assert_eq!(term, (pages / 8).min(32), "{pages} pages");
        let room = stagger_room(geometry, class.size());
        let size = class.size() - room;
        let blocks: Vec<NonNull<u8>> = (0..20)
            .map(|_| back.alloc_staggered(class, size).unwrap().ptr)
            .collect();
        let (mut seen, mut slots) = (BTreeSet::new(), BTreeSet::new());
        for &b in &blocks {
            let (slot, into) = place_of(&back, class, b);
            assert_eq!(
                into,
                64 * (slot % 64) + page * (slot % term),
                "{pages} slot {slot}"
            );
            assert!(
                into + size <= class.size() && into < room,
                "{pages} slot {slot}"
            );
            assert_eq!(back.lookup(b.as_ptr()), Some(Kind::Slot(class)));
            assert_eq!(back.size(b.as_ptr()), Some(size));
            assert_eq!(back.room(b.as_ptr()), Some(class.size() - into));
            let usable = back.usable_size(b.as_ptr()).unwrap();
            assert!(
                usable >= size && into + usable <= class.size(),
                "{pages} slot {slot}"
            );
            assert!(usable.wrapping_add(b.addr().get()).is_multiple_of(page));
            // SAFETY: inside the block.
            unsafe {
                b.as_ptr().write(1);
                b.as_ptr().add(usable - 1).write(1);
            }
            seen.insert(b.addr().get() / page % term);
            slots.insert(slot % term);
        }
        assert_eq!(seen.len(), slots.len(), "{pages}: pages' numbers apart");
        for &b in &blocks {
            back.free_by_address(b);
        }
    }
}

on_every_kernel!(a_large_block, blocks_stagger_by_lines_and_pages);

/// A free slot is charged the pages a tenant may have written, never those a stagger passed over.
/// They stay clean from the carve on, through a claim and a free, until a tenant at the slot's
/// start can write them, and then they are charged until they go back.
fn the_pages_a_stagger_passes_over_stay_clean<S: Shape>() {
    if !room_for::<S>() {
        return;
    }
    let state = state::<S>();
    let back = state.get_or_reserve().unwrap();
    let page = S::GEOMETRY.page_size();
    let class = large(64 * page);
    // Slots 0 to 4 stay held, so that slot 5 is the one that comes back.
    let held: Vec<_> = (0..5)
        .map(|_| back.alloc_staggered(class, KIB).unwrap().ptr)
        .collect();
    let size = 8 * page;
    let first = back.alloc_staggered(class, size).unwrap().ptr;
    assert_eq!(place_of(&back, class, first), (5, 5 * 64 + 5 * page));
    paint(first.as_ptr(), size, 5);
    let start = first.as_ptr().wrapping_sub(5 * 64 + 5 * page);
    let start = NonNull::new(start).unwrap();
    let base = back.retained_large();
    // The block spills into a ninth page, and no other is written.
    let written = 9 * page;
    back.free_by_address(first);
    assert_eq!(back.retained_large() - base, written, "a fresh slot's free");
    assert_eq!(not_clean(&back, class, first), written);
    // Taken and freed again at the same stagger: the count leaves and comes back the same.
    let again = back.alloc_staggered(class, size).unwrap().ptr;
    assert_eq!(again, first);
    assert_eq!(back.retained_large(), base, "a claim takes its charge off");
    back.free_by_address(again);
    assert_eq!(back.retained_large() - base, written, "a claim's free");
    // Grown in place, it is charged what it grew over, and still nothing below it.
    let again = back.alloc_staggered(class, size).unwrap().ptr;
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(again, 16 * page) };
    back.free_by_address(again);
    assert_eq!(back.retained_large() - base, 17 * page, "grown in place");
    // A tenant at the slot's start may write the pages below: its claim clears them, up to its
    // extent, which counts from the stagger as every extent does.
    let plain = back.alloc(class, size).unwrap().ptr;
    assert_eq!(plain, start);
    paint(plain.as_ptr(), size, 6);
    back.free_by_address(plain);
    assert_eq!(
        back.retained_large() - base,
        22 * page,
        "a tenant at the start"
    );
    // A staggered tenant over them leaves them charged, and its claim's trim returns what lay past
    // both its extent and the last tenant's.
    let over = back.alloc_staggered(class, size).unwrap().ptr;
    assert_eq!(over, first);
    assert_eq!(back.retained_large(), base);
    back.free_by_address(over);
    assert_eq!(back.retained_large() - base, 14 * page);
    // A purge returns exactly that, and leaves the whole slot reading as zero.
    assert_eq!(back.trim(), 14 * page);
    assert_eq!(back.retained_large(), base);
    let whole = back.alloc(class, class.size()).unwrap();
    assert_eq!(whole.ptr, start);
    assert!(whole.zeroed);
    assert!(painted(whole.ptr.as_ptr(), class.size(), 0));
    drop(held);
}

on_every_kernel!(
    a_free_slots_charge,
    the_pages_a_stagger_passes_over_stay_clean
);

/// A block that outgrows a staggered slot and is not kept returns the whole slot, from its start,
/// so that the slot can say it reads as zero: an earlier tenant at the slot's start may have
/// written the pages the block's stagger passed over.
fn a_moved_block_returns_what_lay_below_it<S: Shape>() {
    if !room_for::<S>() {
        return;
    }
    let state = state::<S>();
    let back = state.get_or_reserve().unwrap();
    spend_kept_budget(&back);
    let (old, new) = (large(8 * MIB), large(16 * MIB));
    let len = 4 * MIB;
    let held: Vec<_> = (0..3)
        .map(|_| back.alloc_staggered(old, KIB).unwrap().ptr)
        .collect();
    let plain = back.alloc(old, 2 * MIB).unwrap().ptr;
    paint(plain.as_ptr(), 2 * MIB, 9);
    back.free_by_address(plain);
    let block = back.alloc_staggered(old, len).unwrap().ptr;
    let (slot, into) = place_of(&back, old, block);
    assert_eq!(slot, 3);
    assert!(into > S::GEOMETRY.page_size(), "past a page: {into}");
    paint(block.as_ptr(), len, 7);
    let to = back.alloc_staggered(new, 2 * len).unwrap();
    // SAFETY: both are live blocks that nothing else uses; `to` reads as zero if it says so.
    unsafe { back.move_large(block, to.ptr, len, to.zeroed, Move::Outgrown) };
    assert!(painted(to.ptr.as_ptr(), len, 7));
    let again = back.alloc(old, old.size()).unwrap();
    assert_eq!(again.ptr, plain);
    assert!(again.zeroed, "a slot returned whole is known zero");
    assert!(painted(again.ptr.as_ptr(), old.size(), 0));
    drop(held);
}

on_every_kernel!(a_move, a_moved_block_returns_what_lay_below_it);

/// A slot of `class`, staggered past a page and aligned to a huge page, grown in place over one
/// huge page and so advised for them, with its spill set aside and written back (`unspill`).
fn an_advised_block<S: Shape>(back: &Back<'_, Sim<S>>, class: Class) -> NonNull<u8> {
    let (huge, page) = (S::GEOMETRY.huge_page_size(), S::GEOMETRY.page_size());
    let held: Vec<_> = (0..3)
        .map(|_| back.alloc_staggered(class, KIB).unwrap().ptr)
        .collect();
    let block = back.alloc_staggered(class, huge).unwrap().ptr;
    let (slot, into) = place_of(back, class, block);
    assert_eq!(slot, 3);
    assert!(into > page, "past a page: {into}");
    let usable = back.usable_size(block.as_ptr()).unwrap();
    paint(block.as_ptr(), usable, 4);
    let os = back.space().os();
    let purges = os.calls(Call::Purge);
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(block, 2 * huge) };
    assert_eq!(
        os.calls(Call::Purge) - purges,
        1,
        "the spill's huge page went back"
    );
    assert!(
        painted(block.as_ptr(), usable, 4),
        "and its bytes came back"
    );
    let next = block.map_addr(|a| a.saturating_add(huge));
    assert_eq!(
        os.huge_pages(next),
        Some(HugePages::Prefer),
        "the spill's huge page"
    );
    drop(held);
    block
}

/// In a slot advised for huge pages a fault makes the pages below a block resident, so they are
/// charged: where the purge at its free is refused, the free counts them.
fn an_advised_slot_is_charged_from_its_start<S: Shape>() {
    if !room_for::<S>() {
        return;
    }
    let state = state::<S>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = S::GEOMETRY.huge_page_size();
    let class = large(4 * huge);
    let block = an_advised_block(&back, class);
    os.set_faults(Call::Purge, Faults::ALWAYS);
    let before = back.retained_large();
    back.free_by_address(block);
    os.set_faults(Call::Purge, Faults::NONE);
    assert_eq!(back.retained_large() - before, 3 * huge);
}

#[test]
#[cfg_attr(miri, ignore)]
fn an_advised_slot_is_charged_from_its_start_at_2_mib() {
    an_advised_slot_is_charged_from_its_start::<Kernel4k>();
}

#[test]
#[cfg_attr(miri, ignore)]
fn an_advised_slot_is_charged_from_its_start_at_32_mib() {
    an_advised_slot_is_charged_from_its_start::<Kernel16k>();
}

/// A block under advice for huge pages larger than a piece moves a huge page at a time, since a
/// piece's purge would return nothing of one, and returns its whole slot, which reads as zero.
#[test]
#[cfg_attr(miri, ignore)]
fn an_advised_block_moves_in_whole_huge_pages() {
    if !room_for::<Kernel16k>() {
        return;
    }
    let state = state::<Kernel16k>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Kernel16k::GEOMETRY.huge_page_size();
    let class = large(4 * huge);
    let block = an_advised_block(&back, class);
    let start = NonNull::new(block.as_ptr().wrapping_sub(place_of(&back, class, block).1)).unwrap();
    let to = back.alloc_staggered(large(16 * huge), 4 * huge).unwrap();
    let before = back.retained_large();
    let purges = os.calls(Call::Purge);
    let mut pieces = 0;
    // SAFETY: both are live blocks that nothing else uses; `to` reads as zero if it says so.
    unsafe {
        back.move_large_with(block, to.ptr, huge, to.zeroed, Move::Outgrown, || {
            pieces += 1;
        });
    }
    assert!(painted(to.ptr.as_ptr(), huge, 4));
    assert!(pieces <= 2, "{pieces} pieces of a huge page");
    assert!(
        os.calls(Call::Purge) - purges <= 3,
        "a purge a piece, and the rest"
    );
    assert_eq!(
        back.retained_large(),
        before,
        "it went back, and is not counted"
    );
    let again = back.alloc(class, class.size()).unwrap();
    assert_eq!(again.ptr, start);
    assert!(again.zeroed, "every page went back");
}

/// A zeroed block its tenant left unwritten goes back whole at its free, staggered or not, and one
/// its tenant wrote does not.
fn the_unwritten_look_finds_what_a_staggered_block_wrote<S: Shape>() {
    if !room_for::<S>() {
        return;
    }
    let state = state::<S>();
    let back = state.get_or_reserve().unwrap();
    let geometry = back.space().plan().geometry();
    let size = super::super::policy::unwritten_floor(geometry);
    assert_eq!(size, 16 * MIB);
    let class = large(size + stagger_room(geometry, size));
    let held: Vec<_> = (0..3)
        .map(|_| back.alloc_staggered(class, KIB).unwrap().ptr)
        .collect();
    let block = back.alloc_staggered(class, size).unwrap();
    assert!(block.zeroed);
    back.mark_unwritten(block.ptr);
    let before = back.retained_large();
    back.free_by_address(block.ptr);
    assert_eq!(
        back.retained_large(),
        before,
        "left unwritten, it went back"
    );
    let again = back.alloc_staggered(class, size).unwrap();
    assert_eq!(again.ptr, block.ptr);
    assert!(again.zeroed);
    back.mark_unwritten(again.ptr);
    paint(again.ptr.as_ptr(), size, 1);
    back.free_by_address(again.ptr);
    assert!(back.retained_large() >= before + size, "written, it stays");
    drop(held);
}

on_every_kernel!(
    an_unwritten_block,
    the_unwritten_look_finds_what_a_staggered_block_wrote
);

/// A heartbeat's look at the hold returns the pages of a huge unit at most, 512 on every kernel, in
/// a class of a kernel's huge page or more too: the bound is on time.
fn a_look_returns_a_bounded_number_of_pages<S: Shape>() {
    if !room_for::<S>() {
        return;
    }
    let state = floored_state::<S>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let geometry = back.space().plan().geometry();
    let bound = hold_beat(geometry);
    assert_eq!(bound, 512 * geometry.page_size());
    let class = large(48 * MIB);
    let blocks: Vec<_> = (0..4)
        .map(|_| back.alloc(class, class.size()).unwrap().ptr)
        .collect();
    let _top = back.alloc(class, class.size()).unwrap().ptr;
    let beat_for = |n: u32| {
        for _ in 0..n {
            os.advance_clock(FINE_TICKS);
            back.heartbeat();
        }
    };
    beat_for(2);
    for &b in &blocks {
        back.free_by_address(b);
    }
    assert_eq!(back.retained_large(), 4 * class.size());
    beat_for(HOLD_STEPS);
    let mut largest = 0;
    for _ in 0..512 {
        let before = back.retained_large();
        back.heartbeat();
        let returned = before.saturating_sub(back.retained_large());
        if returned == 0 {
            break;
        }
        largest = largest.max(returned);
    }
    assert!(largest > 0, "the hold has ended");
    assert!(
        largest <= bound,
        "one beat returned {largest} of a bound of {bound}"
    );
    assert!(back.retained_large() <= allowance_floor(geometry) + bound);
}

on_every_kernel!(the_holds_look, a_look_returns_a_bounded_number_of_pages);

/// A class's commits run ahead by 64 KiB, or a page if that is more, at the least, and by what the
/// region has committed up to 64 MiB, in pages of any size.
fn a_commit_runs_ahead_by_bytes<S: Shape>() {
    if !room_for::<S>() {
        return;
    }
    let state = state::<S>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let page = S::GEOMETRY.page_size();
    let class = small(2 * KIB);
    let host = back.host(class.class()).unwrap();
    let first = fill(&back, class, 1);
    let region = back.space().locate(first[0]).unwrap().region;
    let committed = || {
        let grow = super::super::layout::Grow::unpack(host.grow(region).load());
        grow.committed as usize * page
    };
    let origin = back.space().plan().slot_origin(class.class());
    let ahead = committed() - (origin + class.size()).next_multiple_of(page);
    assert_eq!(ahead, (64 * KIB).max(page));
    let calls = os.calls(Call::Commit);
    let mut used = class.size();
    let mut most = 0usize;
    while used < 200 * MIB {
        let _ = fill(&back, class, MIB / class.size());
        used += MIB;
        most = most.max(committed() - origin - used);
    }
    assert!(most <= 64 * MIB + page, "{most} ahead");
    assert!(most >= 32 * MIB, "{most} ahead");
    // The logarithm of the region's use, not its use over a step: a call for the slots and a
    // call for the leaf bitmap's pages, about doubling each time and then 64 MiB at a time.
    assert!(
        os.calls(Call::Commit) - calls <= 2 * 24,
        "{}",
        os.calls(Call::Commit) - calls
    );
}

on_every_kernel!(a_commit, a_commit_runs_ahead_by_bytes);

/// The class of 1 KiB, carved past its huge-page threshold, with one live slot left in every eight
/// pages' worth and its frontier aged: huge pages above the frontier mostly free, which a step that
/// may return memory would split. Returns the live slots.
fn stranded<S: Shape>(back: &Back<'_, Sim<S>>) -> Vec<*mut u8> {
    let huge = S::GEOMETRY.huge_page_size();
    let class = small(KIB);
    let slots = carve_to(back, class, 8 * huge + 4 * huge);
    let stride = 8 * S::GEOMETRY.page_size() / KIB;
    let (mut kept, mut freed) = (Vec::new(), Vec::new());
    for (i, &slot) in slots.iter().enumerate() {
        if i.is_multiple_of(stride) {
            kept.push(slot);
        } else {
            freed.push(slot);
        }
    }
    // SAFETY: the test holds these slots and uses none of them.
    unsafe { back.launder(class, &freed) };
    forget_reuse(back);
    back.space().os().advance_clock(DECAY_TICKS);
    kept
}

/// Heartbeats do not split a huge page over 2 MiB, nor does a growth payment, though either would
/// return the memory; the background thread's steps do, and `collect`. `maintain`'s step asks for
/// one at most.
#[test]
#[cfg_attr(miri, ignore)]
fn a_request_never_splits_a_huge_page_over_two_mebibytes() {
    if !room_for::<Kernel16k>() {
        return;
    }
    let state = state::<Kernel16k>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let kept = stranded(&back);
    for _ in 0..BEATS {
        back.heartbeat();
    }
    assert_eq!(os.calls(Call::SplitHuge), 0, "a heartbeat split");
    let _ = fill(&back, small(512), 8 * MIB / 512);
    for _ in 0..BEATS {
        back.heartbeat();
    }
    assert_eq!(os.calls(Call::SplitHuge), 0, "a growth payment split");
    let before = back.retained();
    os.advance_clock(DECAY_TICKS);
    loop {
        let splits = os.calls(Call::SplitHuge);
        let more = back.decay_pass_step(&|| back.now()).more;
        assert!(
            os.calls(Call::SplitHuge) - splits <= 1,
            "a step of maintain's split twice"
        );
        if !more {
            break;
        }
    }
    assert!(os.calls(Call::SplitHuge) > 0, "maintain's steps split");
    os.advance_clock(DECAY_TICKS);
    while back.thread_purge_steps(&|| back.now()).more {}
    assert!(back.retained() < before, "the thread returned it");
    let splits = os.calls(Call::SplitHuge);
    back.trim();
    assert!(os.calls(Call::SplitHuge) >= splits);
    drop(kept);
}

/// On a kernel whose huge page is 2 MiB a heartbeat may split, one a step, as on x86-64.
#[test]
#[cfg_attr(miri, ignore)]
fn a_heartbeat_splits_a_huge_page_of_two_mebibytes() {
    if !room_for::<Kernel4k>() {
        return;
    }
    let state = state::<Kernel4k>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let kept = stranded(&back);
    for _ in 0..16 {
        let splits = os.calls(Call::SplitHuge);
        back.heartbeat();
        assert!(
            os.calls(Call::SplitHuge) - splits <= 1,
            "a heartbeat split twice"
        );
    }
    for _ in 0..BEATS {
        back.heartbeat();
    }
    assert!(os.calls(Call::SplitHuge) > 0);
    drop(kept);
}

/// `Geometry::DESIGN`'s layouts have no page term, on any target, and the stagger is what it always
/// was.
#[test]
fn the_designs_blocks_stagger_by_lines_alone() {
    let plan = Geometry::DESIGN.largest_plan();
    let layout = super::super::layout::Layout::new(plan, large(64 * MIB)).unwrap();
    for slot in 0..200 {
        assert_eq!(layout.stagger(slot), (slot as usize % 64) * 64);
    }
}
