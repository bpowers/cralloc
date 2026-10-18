//! The back end's policy: thresholds, budgets and step sizes, all in one place.
//!
//! They are stated in pages and huge pages so that the simulated heaps, whose pages are
//! smaller, scale with them. The comment on each gives its value in the design's geometry (4 KiB
//! pages, 2 MiB huge pages) and why it has that value. The memory-side values come from
//! `docs/research/memory.md`; none has been tuned against speed yet.

use crate::space::Geometry;

/// The cap on retained memory of small classes, in huge pages: 32, or 64 MiB. Retained memory
/// is free memory in the bitmaps that is believed resident. The cap is the pressure valve for
/// a burst of frees inside one decay interval; the clocked pass is what returns memory in the
/// ordinary case, so the cap can be generous, and a generous cap keeps a program that frees
/// and reallocates tens of megabytes from faulting them back each time.
pub const RETAINED_CAP_HUGE_PAGES: usize = 32;

/// While the small-class count stays over its cap (fragmentation can keep it there), a free
/// tries again each time the count has grown by another `cap / CAP_REARM`: 4, so every 16 MiB
/// of further frees. Without this the cap would act once and never again; with a smaller step
/// a fragmented heap would pay a bounded walk too often.
pub const CAP_REARM: usize = 4;

/// The cap on retained memory of large classes, in huge pages: 4, or 8 MiB. A freed large
/// slot can serve only its own class, so keeping many of them resident buys little, and a
/// buffer that grows by moving leaves a trail of them: with 64 MiB a 64 MiB vector peaked
/// 11 MiB over jemalloc, with 8 MiB it peaked under. Past the cap, a free purges the slot it
/// frees, which is one system call and no search.
pub const LARGE_RETAINED_CAP_HUGE_PAGES: usize = 4;

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

/// How much bitmap the free that pushes the small-class count past its cap (or past the next
/// re-arm step) may read while it purges toward the cap, in words: 16384, 128 KiB. The bound
/// keeps one call from paying for a whole heap; the clocked pass finishes the job.
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

/// The retained-memory cap of small classes in bytes for a geometry.
#[inline(always)]
pub const fn retained_cap(geometry: Geometry) -> usize {
    RETAINED_CAP_HUGE_PAGES * geometry.huge_page_size()
}

/// The piece a moving large block is copied in, in bytes, for a geometry.
#[inline(always)]
pub const fn move_piece(geometry: Geometry) -> usize {
    MOVE_PIECE_HUGE_PAGES * geometry.huge_page_size()
}

/// The retained-memory cap of large classes in bytes for a geometry.
#[inline(always)]
pub const fn large_retained_cap(geometry: Geometry) -> usize {
    LARGE_RETAINED_CAP_HUGE_PAGES * geometry.huge_page_size()
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
