//! `Back::unspill`: a block that grows in place over the huge page holding its old end has that
//! page returned and its first page written back, so that the page faults whole.
//!
//! A growth keeps every byte of the block, also when the purge fails
//! (`a_purge_that_fails_part_way_keeps_the_bytes`). The rest of these tests hold its cost:
//!
//! - The OS is asked whether the rest of the page is resident, and a resident page stays. In a
//!   slot kept resident from its last tenant (`policy::kept_outgrown`), or one that shrank and
//!   grows back, the rest is resident, on Linux as a huge page already, and returning it would
//!   throw it away and fault it back. Measured through the preload library on Linux 7.2.9
//!   (`crates/cralloc-preload/tests/programs/unspill_probe.c`, 2026-10-19): returned regardless,
//!   every in-place doubling of a buffer grown and freed over and over takes one huge-page fault
//!   inside `realloc`. With transparent huge pages off for the process it drops 2 MiB of resident
//!   memory, and the program faults 511 small pages back.
//! - The page is set aside where the advice does not reach. One huge page further on is the
//!   block's next spill only when the block grows by one huge page. A block that doubles from two
//!   huge pages or more has that page in the middle of its new room, inside the range just
//!   advised, so the copy would fault a whole huge page the program has not reached. The same
//!   probe: growing 4 MiB to 8 MiB that way makes 4 MiB resident inside `realloc`, before the
//!   program writes a byte.

use super::*;

/// A block of `size` bytes in slot 1 of `class`, which is staggered, with slot 0 held so that
/// it stays slot 1's when it is freed and allocated again.
fn second_slot<O: Os>(back: &Back<'_, O>, class: Class, size: usize) -> NonNull<u8> {
    let page = back.space().plan().geometry().page_size();
    let block = back.alloc_staggered(class, size).unwrap().ptr;
    if !block.addr().get().is_multiple_of(page) {
        return block;
    }
    let second = back.alloc_staggered(class, size).unwrap().ptr;
    assert!(
        !second.addr().get().is_multiple_of(page),
        "slot 1 is staggered"
    );
    second
}

/// Fills all the block may use, grows it in place to `size`, and returns how many purges the
/// growth made.
fn fill_and_grow<O: Shape>(
    back: &Back<'_, Sim<O>>,
    os: &Sim<O>,
    block: NonNull<u8>,
    size: usize,
) -> u64 {
    pattern(block.as_ptr(), back.usable_size(block.as_ptr()).unwrap());
    let purges = os.calls(Call::Purge);
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(block, size) };
    os.calls(Call::Purge) - purges
}

/// A slot its last tenant left resident (a plain free of a block written whole) holds, for the next
/// block to grow there, the huge page past its old end resident already, whole; returning it frees
/// no page table worth freeing and costs the fault it was meant to save.
#[test]
#[cfg_attr(miri, ignore)]
fn a_block_regrown_in_a_kept_slot_keeps_its_resident_huge_page() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let class = large_of(4 * huge);
    let first = second_slot(&back, class, huge);
    assert_eq!(
        fill_and_grow(&back, os, first, 2 * huge),
        1,
        "in a fresh slot the spilled page is returned, to fault whole"
    );
    // An advised block's free returns its slot; a tenant written whole and not grown leaves
    // it resident at its free.
    back.free(first, class);
    let whole = back.alloc(class, class.size()).unwrap().ptr;
    assert_eq!(
        whole.addr().get() / 4096,
        first.addr().get() / 4096,
        "the same slot"
    );
    pattern(whole.as_ptr(), class.size());
    let purges = os.calls(Call::Purge);
    back.free(whole, class);
    assert_eq!(os.calls(Call::Purge), purges, "the slot is left resident");
    let again = back.alloc_staggered(class, huge).unwrap().ptr;
    assert_eq!(again, first);
    // The huge page the block spills into, resident from the last tenant.
    let spilled = again.map_addr(|a| a.saturating_add(huge - a.get() % huge));
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    assert!(!unsafe { os.is_purged(spilled, huge) });
    assert_eq!(
        fill_and_grow(&back, os, again, 2 * huge),
        0,
        "the growth returned a huge page that was resident whole"
    );
    back.free(again, class);
}

/// The same without a free: a block that shrinks in place inside its advised slot keeps the huge
/// page its new end lies in (the tail goes back in whole huge pages only), and growing back must
/// not return that resident page and fault it again, at every round.
#[test]
#[cfg_attr(miri, ignore)]
fn a_block_that_shrinks_and_grows_back_keeps_its_resident_huge_page() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let class = large_of(4 * huge);
    let block = second_slot(&back, class, huge);
    assert_eq!(fill_and_grow(&back, os, block, 2 * huge), 1);
    for round in 0..3 {
        pattern(block.as_ptr(), back.usable_size(block.as_ptr()).unwrap());
        let purges = os.calls(Call::Purge);
        // SAFETY: the test holds the block, uses none of it during the call, and gives up
        // what lies past the new size.
        unsafe { back.resize_in_place(block, huge) };
        assert_eq!(
            os.calls(Call::Purge),
            purges,
            "the shrink keeps the huge page"
        );
        assert_eq!(
            fill_and_grow(&back, os, block, 2 * huge),
            0,
            "round {round}: the growth returned a huge page that was resident whole"
        );
    }
    back.free(block, class);
}

/// The new room of a block that grows in place is the program's to write, and huge-page advice
/// over it costs nothing until it does. A page set aside one huge page past the old end would,
/// when the block doubles from two huge pages, lie inside the range just advised, and on Linux the
/// copy would fault that huge page whole.
#[test]
#[cfg_attr(miri, ignore)]
fn a_growth_writes_no_advised_huge_page_the_program_has_not_reached() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let page = Native::GEOMETRY.page_size();
    let class = large_of(8 * huge);
    let block = second_slot(&back, class, 2 * huge);
    let before = back.usable_size(block.as_ptr()).unwrap();
    assert_eq!(fill_and_grow(&back, os, block, 4 * huge), 1);
    let after = back.usable_size(block.as_ptr()).unwrap();
    // The huge page the block had begun, which it faults back, is its own to write.
    let begun = (block.addr().get() + before - 1) & !(huge - 1);
    for offset in (before..after).step_by(page) {
        let at = block.map_addr(|a| a.saturating_add(offset));
        if at.addr().get() & !(huge - 1) == begun {
            continue;
        }
        if os.huge_pages(at) == Some(HugePages::Prefer) {
            // The slot is fresh, so a page nothing wrote reads as zero.
            assert!(
                painted(at.as_ptr(), page, 0),
                "the growth wrote the advised huge page {} huge pages into the block",
                offset / huge
            );
        }
    }
    back.free(block, class);
}

/// The path the measured workload takes: the block moves into its slot as it grows, so it is
/// advised, and its clean bits are cleared to the end of the huge page it spills into
/// (`region::cover` rounds an advised block's extent up), before it ever grows in place. The
/// spilled page is still returned. Asking the clean bits whether the rest of the page is resident
/// passes the tests above and fails this one: on Linux it never returns the page at all.
#[test]
#[cfg_attr(miri, ignore)]
fn a_block_that_moved_in_returns_its_spilled_page_when_it_grows_in_place() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let class = large_of(8 * huge);
    let block = second_slot(&back, class, 2 * huge);
    move_in(&back, large_of(huge), huge, block, 2 * huge);
    assert_eq!(os.huge_pages(block), Some(HugePages::Prefer));
    assert_eq!(fill_and_grow(&back, os, block, 4 * huge), 1);
    assert!(patterned(
        block.as_ptr(),
        back.usable_size(block.as_ptr()).unwrap().min(2 * huge)
    ));
    back.free(block, class);
}

/// A purge that the OS refuses, or that fails after acting on half the huge page, loses
/// nothing: the first page is written back whatever the purge did.
#[test]
#[cfg_attr(miri, ignore)]
fn a_purge_that_fails_part_way_keeps_the_bytes() {
    for faults in [Faults::ALWAYS, Faults::ALWAYS.partially()] {
        let state = state::<Native>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let huge = Native::GEOMETRY.huge_page_size();
        let class = large_of(4 * huge);
        let block = second_slot(&back, class, huge);
        let usable = back.usable_size(block.as_ptr()).unwrap();
        os.set_faults(Call::Purge, faults);
        assert_eq!(fill_and_grow(&back, os, block, 2 * huge), 1);
        os.set_faults(Call::Purge, Faults::NONE);
        assert!(patterned(block.as_ptr(), usable), "{faults:?}");
        back.free(block, class);
    }
}

/// The edges of where the page is set aside: old and new sizes whose ends fall just before, on and
/// just past page and huge-page boundaries, up to the slot's end. Whatever the growth does, the
/// block keeps every byte it could use, the growth returns one huge page at most, and no advised
/// huge page of the new room is written before the program reaches it.
#[test]
#[cfg_attr(miri, ignore)]
fn a_growth_at_any_boundary_keeps_the_bytes_and_writes_no_advised_room() {
    let huge = Native::GEOMETRY.huge_page_size();
    let page = Native::GEOMETRY.page_size();
    let class = large_of(8 * huge);
    // Offsets from a boundary, for the end of a block that starts a cache line into its slot.
    let near = |at: usize| {
        [
            at - page,
            at - 64,
            at - 1,
            at,
            at + 1,
            at + page - 64,
            at + page,
        ]
    };
    let (mut cases, mut returned) = (0, 0);
    for old in [2, 3].into_iter().flat_map(|n| near(n * huge)) {
        for new in (3..=8).flat_map(|n| near(n * huge)) {
            let state = state::<Native>();
            let back = state.get_or_reserve().unwrap();
            let os = state.os();
            let block = second_slot(&back, class, old);
            let room = back.room(block.as_ptr()).unwrap();
            if new <= old || new > 2 * old || new > room {
                continue;
            }
            let before = back.usable_size(block.as_ptr()).unwrap();
            let purges = fill_and_grow(&back, os, block, new);
            assert!(purges <= 1, "{old} to {new}: {purges} purges");
            assert!(
                patterned(block.as_ptr(), before),
                "{old} to {new}: the block lost bytes"
            );
            let after = back.usable_size(block.as_ptr()).unwrap();
            assert!(after <= room);
            let begun = (block.addr().get() + before - 1) & !(huge - 1);
            for offset in (before..after).step_by(page) {
                let at = block.map_addr(|a| a.saturating_add(offset));
                if at.addr().get() & !(huge - 1) != begun
                    && os.huge_pages(at) == Some(HugePages::Prefer)
                {
                    assert!(
                        painted(at.as_ptr(), page, 0),
                        "{old} to {new}: the growth wrote advised room at {offset}"
                    );
                }
            }
            // The program can fill all it was given, and free it.
            pattern(block.as_ptr(), after);
            back.free(block, class);
            cases += 1;
            returned += purges;
        }
    }
    // The sweep reaches both outcomes: growths that return the page and growths that do not.
    assert!(
        cases > 50 && returned > 5 && returned < cases,
        "{returned} of {cases}"
    );
}
