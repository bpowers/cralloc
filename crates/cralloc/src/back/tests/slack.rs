//! The trim at the claim of a free large slot (`Back::trim_slack`): what it returns and what it
//! keeps, that the resident part of a hot slot no longer climbs, the slots that have nothing to
//! trim (advised for huge pages, left unwritten), a kept outgrown slot, the count at the claim,
//! and a refused or half-done call.

use super::*;

/// The bytes from a slot's start to the end of the page holding a non-staggered block of `size`
/// bytes in slot 0 of a region: its extent.
fn extent(size: usize) -> usize {
    size.next_multiple_of(Native::GEOMETRY.page_size())
}

/// Whether bytes `from..to` of the block at `ptr` have been returned and read as zero.
fn returned(os: &SimOs<Native>, ptr: NonNull<u8>, from: usize, to: usize) -> bool {
    // SAFETY: the test's own blocks, or free slots, which nothing else uses meanwhile.
    from >= to || unsafe { os.is_purged(ptr.map_addr(|a| a.saturating_add(from)), to - from) }
}

/// A claim keeps the room its slot's last tenant used and returns, at once, what tenants before
/// that one left past it; nothing below. The count loses the slot's whole charge at the claim,
/// and the next free charges what the slot has left resident.
#[test]
#[cfg_attr(miri, ignore)]
fn a_claim_returns_what_the_tenants_before_the_last_left() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(512 << 10);
    let size = class.size();
    let a = back.alloc(class, size).unwrap().ptr;
    paint(a.as_ptr(), size, 1);
    back.free(a, class);
    // The next tenant is smaller, and keeps the whole of the last one's room.
    let purges = os.calls(Call::Purge);
    let b = back.alloc(class, 100 << 10).unwrap();
    assert_eq!(b.ptr, a);
    assert_eq!(
        os.calls(Call::Purge),
        purges,
        "the last tenant's room is kept"
    );
    assert!(painted(a.as_ptr(), size, 1));
    assert_eq!(
        back.retained_large(),
        0,
        "the claim took the slot's charge off"
    );
    paint(a.as_ptr(), 100 << 10, 2);
    back.free(a, class);
    assert_eq!(
        back.retained_large(),
        size,
        "the first tenant's tail is still resident"
    );
    // The one after keeps what the second used and returns what only the first wrote.
    let c = back.alloc(class, 40 << 10).unwrap();
    assert_eq!(c.ptr, a);
    assert_eq!(
        os.calls(Call::Purge),
        purges + 1,
        "one call returns the slack"
    );
    assert!(returned(os, a, extent(100 << 10), size));
    assert!(painted(
        a.as_ptr().wrapping_add(40 << 10),
        (100 - 40) << 10,
        2
    ));
    assert_eq!(back.retained_large(), 0);
    back.free(a, class);
    assert_eq!(
        back.retained_large(),
        extent(100 << 10),
        "the free charges what the slot kept"
    );
}

/// Inside a slot that tenants of many sizes keep taking, what is resident after each claim lies
/// within the larger of the new block's extent and the last tenant's: the high-water mark of
/// every tenant since the slot was last purged no longer stands.
#[test]
#[cfg_attr(miri, ignore)]
fn what_a_hot_slot_holds_no_longer_climbs() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = large(512 << 10);
    let size = class.size();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut last = 0;
    let mut slot = None;
    for round in 0..200 {
        let want = match rng.below(4) {
            0 => size,
            1 => 1 + rng.below(size),
            _ => 1 + rng.below(size / 8),
        };
        let block = back.alloc(class, want).unwrap().ptr;
        assert_eq!(*slot.get_or_insert(block), block, "one slot");
        let keep = extent(want).max(extent(last)).min(size);
        // Every tenant paints all it asked for, so a zero page was returned or never written.
        assert!(
            painted(block.as_ptr().wrapping_add(keep), size - keep, 0),
            "round {round}: {want} after {last} left pages past {keep} resident"
        );
        paint(block.as_ptr(), want, round as u8 | 1);
        back.free(block, class);
        last = want;
    }
}

/// A slot advised for huge pages goes back whole at its free, and one that held a block left
/// mostly unwritten likewise; a marked block that wrote its whole extent is an ordinary free,
/// but its slot had nothing resident past that extent, since the block got it all clean. None
/// of them has anything for the next claim to trim, and the claim makes no call.
#[test]
#[cfg_attr(miri, ignore)]
fn slots_returned_whole_or_written_only_by_their_tenant_have_nothing_to_trim() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let _ballast = spend_kept_budget(&back);
    let os = state.os();
    let huge = Native::GEOMETRY.huge_page_size();
    // Advised: a block that doubled in place over a huge page, densely.
    let class = large(256 << 10);
    let block = back.alloc(class, huge).unwrap().ptr;
    paint(block.as_ptr(), huge, 3);
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(block, 2 * huge) };
    assert_eq!(os.huge_pages(block), Some(HugePages::Prefer));
    paint(block.as_ptr(), 2 * huge, 3);
    back.free(block, class);
    assert!(
        returned(os, block, 0, class.size()),
        "the free returned it whole"
    );
    let purges = os.calls(Call::Purge);
    let again = back.alloc(class, 40 << 10).unwrap();
    assert_eq!((again.ptr, again.zeroed), (block, true));
    assert_eq!(
        os.calls(Call::Purge),
        purges,
        "the claim of an advised slot trimmed"
    );
    back.free(again.ptr, class);
    // Unwritten: a zeroed block from a zero slot, marked; written in one page, then a smaller
    // one written in full.
    let class = large(512 << 10);
    for (asked, written, whole) in [(class.size(), 4096, true), (300 << 10, 300 << 10, false)] {
        let block = back.alloc(class, asked).unwrap();
        assert!(block.zeroed);
        back.mark_unwritten(block.ptr);
        paint(block.ptr.as_ptr(), written, 4);
        back.free(block.ptr, class);
        assert_eq!(returned(os, block.ptr, 0, class.size()), whole);
        let purges = os.calls(Call::Purge);
        let next = back.alloc(class, 40 << 10).unwrap();
        assert_eq!(next.ptr, block.ptr);
        assert_eq!(os.calls(Call::Purge), purges, "written {written}: trimmed");
        back.free(next.ptr, class);
        // Back to zero for the next case.
        back.trim();
    }
}

/// A slot a block outgrew and the move kept is taken by the next growing block: the claim keeps
/// all the room the block that outgrew it used, which is what keeping it is for, returns only
/// what an earlier, larger tenant left past that, and the count of kept outgrown slots falls by
/// what the keep added, as before the trim.
#[test]
#[cfg_attr(miri, ignore)]
fn a_kept_outgrown_slot_keeps_the_room_its_block_used() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (class, wider) = (large(192 << 10), large(512 << 10));
    let size = class.size();
    // An earlier tenant wrote the whole slot.
    let first = back.alloc(class, size).unwrap().ptr;
    paint(first.as_ptr(), size, 5);
    back.free(first, class);
    // A buffer takes it small, grows in place to 120 KiB, and outgrows it.
    let grown = back.alloc(class, 40 << 10).unwrap().ptr;
    assert_eq!(grown, first);
    // SAFETY: the test holds the block and uses none of it during the call.
    unsafe { back.resize_in_place(grown, 120 << 10) };
    paint(grown.as_ptr(), 120 << 10, 6);
    let to = back.alloc(wider, wider.size()).unwrap();
    back.resize(to.ptr, 240 << 10);
    // SAFETY: both are live blocks the test holds, `grown` given up.
    unsafe {
        back.move_large(
            grown,
            to.ptr,
            120 << 10,
            to.zeroed,
            crate::back::Move::Outgrown,
        )
    };
    assert!(back.kept_outgrown_at(grown.as_ptr()));
    let kept = back.outgrown();
    assert_eq!(kept, size, "the keep counted the slot's charge");
    let before = back.retained_large();
    let purges = os.calls(Call::Purge);
    // The next growing block claims it at its own size, as the heap claims a slot with room.
    let next = back.alloc(class, 40 << 10).unwrap();
    assert_eq!(next.ptr, grown);
    assert_eq!(back.outgrown(), 0, "the claim took the keep's charge off");
    assert_eq!(back.retained_large(), before - size);
    assert_eq!(os.calls(Call::Purge), purges + 1);
    assert!(painted(grown.as_ptr(), 120 << 10, 6), "the outgrown room");
    assert!(returned(os, grown, extent(120 << 10), size));
    back.free(next.ptr, class);
    back.free(to.ptr, wider);
}

/// A trim the kernel refuses, wholly or after returning half its range, leaves the pages as
/// they were in the books: not clean, so the block's free charges them and the next claim tries
/// again; and no clean bit stands over a page that holds data.
#[test]
#[cfg_attr(miri, ignore)]
fn a_refused_trim_leaves_its_pages_counted_and_tries_again() {
    for faults in [Faults::once_after(0), Faults::once_after(0).partially()] {
        let state = state::<Native>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let class = large(512 << 10);
        let size = class.size();
        let a = back.alloc(class, size).unwrap().ptr;
        paint(a.as_ptr(), size, 1);
        back.free(a, class);
        let b = back.alloc(class, 40 << 10).unwrap().ptr;
        back.free(b, class);
        os.set_faults(Call::Purge, faults);
        let c = back.alloc(class, 40 << 10).unwrap().ptr;
        os.set_faults(Call::Purge, Faults::NONE);
        assert_eq!(c, a);
        assert_eq!(
            not_clean(&back, class, c),
            size,
            "{faults:?}: a page the trim did not report returned is marked clean"
        );
        if !faults.partial {
            assert!(painted(
                a.as_ptr().wrapping_add(40 << 10),
                size - (40 << 10),
                1
            ));
        }
        assert_eq!(back.retained_large(), 0);
        back.free(c, class);
        assert_eq!(
            back.retained_large(),
            size,
            "{faults:?}: the free charges them"
        );
        // The next claim, after a tenant of the same size, takes them.
        let d = back.alloc(class, 40 << 10).unwrap().ptr;
        assert!(returned(os, d, extent(40 << 10), size), "{faults:?}");
        back.free(d, class);
        assert_eq!(back.retained_large(), extent(40 << 10));
    }
}

/// A claim returns a purge step of slack at most, the bound of every purge on a request's
/// path, from the top of the slot down; what it leaves stays resident, and the slot's next
/// claim returns the next step. Here a slot of 1 MiB, 256 pages, two steps.
#[test]
#[cfg_attr(miri, ignore)]
fn a_claim_returns_a_step_of_slack_at_most_from_the_top() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(1 << 20);
    let size = class.size();
    let page = Native::GEOMETRY.page_size();
    let step = super::super::policy::PURGE_STEP_PAGES as usize * Native::GEOMETRY.page_size();
    assert!(size > step && size < 2 * step + page);
    let a = back.alloc(class, size).unwrap().ptr;
    paint(a.as_ptr(), size, 1);
    back.free(a, class);
    let b = back.alloc(class, page).unwrap().ptr;
    back.free(b, class);
    // The slack is past the tenant's extent of a page: all but that. One step goes, the top one.
    let c = back.alloc(class, page).unwrap().ptr;
    assert_eq!(c, a);
    assert!(returned(os, a, size - step, size));
    assert!(painted(
        a.as_ptr().wrapping_add(page),
        size - step - page,
        1
    ));
    back.free(c, class);
    assert_eq!(
        back.retained_large(),
        size - step,
        "the free charges what is left"
    );
    // The next claim takes the rest, less than a step, down to the tenants' extent.
    let d = back.alloc(class, page).unwrap().ptr;
    assert!(returned(os, a, page, size));
    back.free(d, class);
    assert_eq!(back.retained_large(), page);
}
