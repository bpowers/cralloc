//! The back end's policy: thresholds, budgets and step sizes, all in one place.
//!
//! They are stated in pages and huge pages so that the simulated heaps, whose pages are
//! smaller, scale with them. The comment on each gives its value in the design's geometry (4 KiB
//! pages, 2 MiB huge pages) and why it has that value. The memory-side values come from
//! `docs/research/memory.md`; none has been tuned against speed yet.

use crate::space::Geometry;

/// The cap on retained memory, small and large classes together, in huge pages: 32, or
/// 64 MiB. Retained memory is free memory in the bitmaps that is believed resident. The cap
/// is enforced when the heap grows, never when it frees: a level cannot tell memory that is
/// idle from memory about to be reused, and purging on frees faulted reused memory straight
/// back (`docs/research/memory.md`). Address space is segregated by class, so free memory of
/// one class serves another only through the kernel; the cap bounds what a phase change can
/// leave resident above what is live. The clocked pass returns memory in the ordinary case.
pub const RETAINED_CAP_HUGE_PAGES: usize = 32;

/// Growth is charged against the cap in steps of this many huge pages: 1, or 2 MiB. Each step
/// of carving, while retained memory is over the cap, pays for at most one bounded purge
/// walk, so a heap whose free memory is all fragmentation pays one walk per step of growth.
/// A step costs at least a huge-page fault or 512 small ones, which dwarfs the walk.
pub const GROWTH_STEP_HUGE_PAGES: usize = 1;

/// The decay interval in ticks of [`crate::os::Os::ticks`]: 2^32, about a second of the cycle
/// counter at a few gigahertz, the same as the idle check's. A pass over the heap starts at
/// most once an interval. A second is long next to the microseconds between heartbeats and
/// short next to the ten seconds over which jemalloc returns everything.
pub const DECAY_TICKS: u64 = 1 << 32;

/// A decay pass may purge the retained count shifted right by this: 2, a quarter. From the
/// top down a quarter an interval returns 90% of what stays free within eight intervals,
/// while memory freed and reused within an interval or two is mostly left alone.
pub const DECAY_SHIFT: u32 = 2;

/// A decay pass may always purge this much, in huge pages: 1, or 2 MiB, so that the tail of a
/// decay ends instead of shrinking by quarters for ever.
pub const DECAY_FLOOR_HUGE_PAGES: usize = 1;

/// How much bitmap a heartbeat's step of a decay pass may read, in words: 256 words, 2 KiB.
/// Each word covers 64 slots, so this is one 4 KiB page of 8-byte slots read eight times over,
/// or a few hundred pages of larger ones. A step always finishes the unit it started, so a
/// huge-page unit of small slots may read more.
pub const PURGE_STEP_WORDS: usize = 256;

/// How much bitmap a step of growth over the cap may read while it purges, in words: 16384,
/// 128 KiB. The bound keeps one call from paying for a whole heap; the clocked pass and later
/// steps finish the job.
pub const PURGE_CAP_WORDS: usize = 1 << 14;

/// A region of a class that gets huge pages marks the rest of itself for them once it has
/// carved more than this many huge pages: 8, or 16 MiB. The first touch of a huge page makes
/// all of it resident, so the page at the watermark costs up to one huge page that the
/// program never asked for: at most an eighth of the class. Measured against 64 MiB, after
/// the rules that move parked objects back to the bitmaps: JSON spends half as long in the
/// kernel (2.4 against 4.5 cycles a byte) for 1.1 MiB more at its peak; 1M-object churn and
/// the burst-then-idle test peak 1.7 and 7 MiB higher (0.7% and 3%); memory after a peak is
/// unchanged. 8 MiB saves a little more time and takes churn 3 MiB over jemalloc
/// (`docs/research/zen5-roofline.md`, section 7). A simulated heap whose regions are smaller
/// than four times this uses a quarter of a region instead.
pub const HUGE_AFTER_HUGE_PAGES: usize = 8;

/// Only classes with at least this many slots to a page get huge pages: 4, so classes up
/// to 1 KiB. Small objects are dense in their pages, so a huge page holds what the program
/// uses. A block of a page or more is often touched in part, and with huge pages every part
/// touched would cost 2 MiB: 256 blocks of 1 MiB with one page written each were 246 MiB
/// resident.
pub const HUGE_MIN_SLOTS_PER_PAGE: usize = 4;

/// A large block that grows to at least this many huge pages, by moving or in place, gets huge
/// pages up to its new size, if the last piece the program wrote holds data in at least half
/// its pages: those of its slot's whole huge pages are advised for them
/// (`region::advise_grown`) until the slot is freed, when they are purged and the advice is
/// taken back. 1, or 2 MiB. Growth by appending is the evidence of dense use, and the bytes
/// written last are checked to be dense; the advice never covers the slot's room past the size
/// asked for, so it runs at most one growth step ahead of what has been shown. A block
/// allocated at its size in one call has shown neither, and with huge pages each part touched
/// would cost 2 MiB. The soak's growing buffers, written a stripe a megabyte past 64 MiB,
/// were 1013 MiB resident with the whole slot advised at each growth, and 271 without.
///
/// Half, because a block that holds data in at least half its pages costs at most twice what
/// it touches when huge pages back it; below that the cost can approach 512 times. One huge
/// page, because a smaller block has no whole huge page to advise, and from one up the advice
/// makes resident at most one huge page that the program did not write.
pub const GROWN_HUGE_MIN_HUGE_PAGES: usize = 1;

/// ...and only if it at most doubles at once: a doubling is the signature of appending. A
/// larger jump is a one-off reserve that says nothing about how densely the buffer will be
/// used, and khugepaged would make every huge page it touches resident whole
/// (`docs/research/zen5-roofline.md`, section 7): a buffer written 1 MiB densely, grown to
/// 4 MiB and then written sparsely kept all 4 MiB resident.
pub const GROWN_HUGE_MAX_FACTOR: usize = 2;

/// A large block whose alignment allows it starts this many bytes past its slot's start for
/// each step of its slot's index, over `STAGGER_SLOTS` slots: 64, a cache line, so that the
/// same offset in large blocks of one class falls in different cache sets. Slots are spaced by
/// their class size, often a power of two, and buffers walked together at slot starts compete
/// for one set of every cache level: 64 buffers of 4 MiB written round-robin took 7.9 ns an
/// element at slot starts and 0.79 staggered (`docs/research/zen5-roofline.md`, section 7).
pub const STAGGER_STEP: usize = 64;

/// The number of distinct staggers: 64, so they span a page, 4 KiB, which is every set of the
/// L1 and 64 sets apart in the L2. The heap gives a large request this much more room, so that
/// any stagger fits; the room costs address space, and at most one more page of memory.
pub const STAGGER_SLOTS: usize = 64;

/// The room a staggered block needs beyond its size: `STAGGER_STEP * STAGGER_SLOTS`, 4 KiB.
pub const STAGGER_ROOM: usize = STAGGER_STEP * STAGGER_SLOTS;

/// A large block that moves is copied in pieces of this many huge pages, and the pages of the
/// old block behind each piece go back to the OS before the next is copied: 1, or 2 MiB. So a
/// moving block has both copies resident for at most one piece, where a single copy had them
/// for its whole size (four threads growing buffers to 256 MiB peaked at three times glibc,
/// which moves by remapping). A piece costs one `madvise` on top of its copy; 2 MiB copies in
/// about a hundred microseconds, so the call is a few percent, and a smaller piece would buy
/// little memory for more calls. A block of at most one piece is copied whole.
pub const MOVE_PIECE_HUGE_PAGES: usize = 1;

/// In the small-page part of a region, each commit makes this many pages writable beyond what
/// the carve that asked for it needs: 16, or 64 KiB. `mprotect` costs about a microsecond, so
/// this makes it rare without charging much address space against strict overcommit.
pub const COMMIT_AHEAD_PAGES: usize = 16;

/// In the huge-page part, each commit makes writable whole huge pages, this many beyond what
/// the carve needs: 1, or 2 MiB, so growth faults a huge page at a time.
pub const COMMIT_AHEAD_HUGE_PAGES: usize = 1;

/// The retained-memory cap in bytes for a geometry.
#[inline(always)]
pub const fn retained_cap(geometry: Geometry) -> usize {
    RETAINED_CAP_HUGE_PAGES * geometry.huge_page_size()
}

/// The growth step in bytes for a geometry.
#[inline(always)]
pub const fn growth_step(geometry: Geometry) -> usize {
    GROWTH_STEP_HUGE_PAGES * geometry.huge_page_size()
}

/// How much a grower that has just taken a growth debt of `debt` bytes purges, with
/// `retained` bytes retained against a cap of `cap`: nothing until the debt is a whole `step`
/// (a racer that took only the crumbs another left does nothing), and then as much as it
/// grew, but no more than is over the cap.
#[inline(always)]
pub const fn growth_goal(debt: usize, retained: usize, cap: usize, step: usize) -> usize {
    if debt < step {
        return 0;
    }
    let over = retained.saturating_sub(cap);
    if debt < over { debt } else { over }
}

/// The piece a moving large block is copied in, in bytes, for a geometry.
#[inline(always)]
pub const fn move_piece(geometry: Geometry) -> usize {
    MOVE_PIECE_HUGE_PAGES * geometry.huge_page_size()
}

/// The most a decay pass may purge when the retained count is `retained` bytes: a quarter of
/// it, or all of it once that is within the floor.
#[inline(always)]
pub const fn decay_goal(geometry: Geometry, retained: usize) -> usize {
    let floor = DECAY_FLOOR_HUGE_PAGES * geometry.huge_page_size();
    let part = retained >> DECAY_SHIFT;
    let least = if retained < floor { retained } else { floor };
    if part > least { part } else { least }
}

/// The huge pages a region of `region_huge_pages` huge pages carves before its growth switches
/// to huge pages.
#[inline(always)]
pub const fn huge_after(region_huge_pages: usize) -> usize {
    let quarter = region_huge_pages / 4;
    if quarter < HUGE_AFTER_HUGE_PAGES {
        quarter
    } else {
        HUGE_AFTER_HUGE_PAGES
    }
}

/// Whether a class of `size`-byte slots gets huge pages where pages are `page` bytes.
#[inline(always)]
pub const fn wants_huge(size: usize, page: usize) -> bool {
    size * HUGE_MIN_SLOTS_PER_PAGE <= page
}

/// Whether a large block growing from `old` to `new` bytes gets huge pages, where huge pages
/// are `huge` bytes.
#[inline(always)]
pub const fn grown_huge(old: usize, new: usize, huge: usize) -> bool {
    new > old
        && new >= GROWN_HUGE_MIN_HUGE_PAGES * huge
        && old.saturating_mul(GROWN_HUGE_MAX_FACTOR) >= new
}
