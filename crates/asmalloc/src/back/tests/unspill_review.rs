//! Review of `Back::unspill` (main `f2f2c68`): a block that grows in place over the huge page
//! holding its old end has that page returned and its first page written back, so that the
//! page faults whole.
//!
//! No way to lose the block's bytes was found; `a_purge_that_fails_part_way_keeps_the_bytes`
//! covers the one case the original test left out. The two findings are of cost; both are
//! fixed (the OS is asked whether the rest of the page is resident, and the page is set aside
//! where the advice does not reach), and their tests pin the fixes:
//!
//! - kept-slot: the page is returned whether or not the rest of it is resident. In a slot kept
//!   resident from its last tenant (`policy::kept_grown`), or one that shrank and grows back,
//!   the rest is resident, on Linux as a huge page already, and the growth throws it away and
//!   faults it back. Measured through the preload library on Linux 7.2.9
//!   (`crates/asmalloc-preload/tests/programs/unspill_probe.c`, 2026-10-19): every in-place
//!   doubling of a buffer grown and freed over and over takes one huge-page fault inside
//!   `realloc` where it took none. With transparent huge pages off for the process it drops
//!   2 MiB of resident memory, and the program faults 511 small pages back where it faulted
//!   none.
//! - aside-ahead: the page is set aside one huge page further on, which the comment calls the
//!   block's next spill. That is so only when the block grows by one huge page. A block that
//!   doubles from two huge pages or more has it in the middle of its new room, inside the range
//!   just advised, so the copy faults a whole huge page the program has not reached. The same
//!   probe: growing 4 MiB to 8 MiB makes 4 MiB resident inside `realloc`, before the program
//!   writes a byte.

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

/// Finding kept-slot. A grown block freed within the budget for kept grown blocks leaves its
/// slot resident, so that a buffer grown again lands on memory it need not fault. The next
/// block to grow there has the huge page past its old end resident already, whole; returning
/// it frees no page table worth freeing and costs the fault it was meant to save.
#[test]
#[cfg_attr(miri, ignore)]
fn a_block_regrown_in_a_kept_slot_keeps_its_resident_huge_page() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let class = large(4 * huge);
    let first = second_slot(&back, class, huge);
    assert_eq!(
        fill_and_grow(&back, os, first, 2 * huge),
        1,
        "in a fresh slot the spilled page is returned, to fault whole"
    );
    let usable = back.usable_size(first.as_ptr()).unwrap();
    pattern(first.as_ptr(), usable);
    let purges = os.calls(Call::Purge);
    back.free(first, class);
    assert_eq!(os.calls(Call::Purge), purges, "the slot is kept resident");
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

/// Finding kept-slot, without a free: a block that shrinks in place inside its advised slot
/// keeps the huge page its new end lies in (the tail goes back in whole huge pages only), and
/// growing back returns that resident page and faults it again, at every round.
#[test]
#[cfg_attr(miri, ignore)]
fn a_block_that_shrinks_and_grows_back_keeps_its_resident_huge_page() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let class = large(4 * huge);
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

/// Finding aside-ahead. The new room of a block that grows in place is the program's to
/// write, and huge-page advice over it costs nothing until it does. The page set aside lies
/// one huge page past the old end; when the block doubles from two huge pages that is inside
/// the range just advised, and on Linux the copy faults that huge page whole.
#[test]
#[cfg_attr(miri, ignore)]
fn a_growth_writes_no_advised_huge_page_the_program_has_not_reached() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let page = Native::GEOMETRY.page_size();
    let class = large(8 * huge);
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

/// The path the measured workload takes, which the original test does not: the block moves
/// into its slot as it grows, so it is advised, and its clean bits are cleared to the end of
/// the huge page it spills into (`region::cover` rounds an advised block's extent up), before
/// it ever grows in place. The spilled page is still returned. A fix of kept-slot that asks
/// the clean bits whether the rest of the page is resident passes the tests above and fails
/// this one: it was tried, and on Linux it never returned the page at all.
#[test]
#[cfg_attr(miri, ignore)]
fn a_block_that_moved_in_returns_its_spilled_page_when_it_grows_in_place() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    let class = large(8 * huge);
    let block = second_slot(&back, class, 2 * huge);
    move_in(&back, large(huge), huge, block, 2 * huge);
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
        let class = large(4 * huge);
        let block = second_slot(&back, class, huge);
        let usable = back.usable_size(block.as_ptr()).unwrap();
        os.set_faults(Call::Purge, faults);
        assert_eq!(fill_and_grow(&back, os, block, 2 * huge), 1);
        os.set_faults(Call::Purge, Faults::NONE);
        assert!(patterned(block.as_ptr(), usable), "{faults:?}");
        back.free(block, class);
    }
}
