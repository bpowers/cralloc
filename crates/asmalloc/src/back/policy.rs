//! The back end's policy: thresholds, budgets and step sizes, all in one place.
//!
//! None of these is tuned; each is a starting point to be measured. They are stated in pages
//! and huge pages so that the simulated heaps, whose pages are smaller, scale with them. The
//! comment on each gives its value in the design's geometry (4 KiB pages, 2 MiB huge pages).

use crate::space::Geometry;

/// The cap on retained memory, in huge pages: 32, or 64 MiB. Retained memory is free memory in
/// the bitmaps that is still resident. When a free pushes it past the cap, that call purges
/// until it is back under (within `PURGE_CAP_WORDS`), and later heartbeats continue while it is
/// over.
pub const RETAINED_CAP_HUGE_PAGES: usize = 32;

/// How much bitmap a heartbeat's purge step may read, in words: 256 words, 2 KiB. Each word
/// covers 64 slots, so this is one 4 KiB page of 8-byte slots read eight times over, or a few
/// hundred pages of larger ones. A step always finishes the unit it started, so a huge-page
/// unit of small slots may read more.
pub const PURGE_STEP_WORDS: usize = 256;

/// How much bitmap the call that pushes retained memory past the cap may read while it brings
/// it back under, in words: 16384, 128 KiB. The design says that call purges until the count
/// is under the cap; this bound keeps one call from paying for a whole heap, and heartbeats
/// finish the job.
pub const PURGE_CAP_WORDS: usize = 1 << 14;

/// A region marks the rest of itself for huge pages once it has carved more than this many
/// huge pages: 4, or 8 MiB. Until then it uses small pages, so a lightly used class costs only
/// the pages it touches.
pub const HUGE_AFTER_HUGE_PAGES: usize = 4;

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

/// The bytes a region carves before its growth switches to huge pages.
#[inline(always)]
pub const fn huge_after(geometry: Geometry) -> usize {
    HUGE_AFTER_HUGE_PAGES * geometry.huge_page_size()
}
