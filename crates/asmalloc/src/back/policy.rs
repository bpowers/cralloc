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

/// The floor of the allowance, in huge pages: 16, or 32 MiB. Memory up to this much is spent
/// freely: the clocked pass returns nothing while what a purge could return (by the last
/// lap's count, `purge`) totals no more than the floor, and stops at the floor when above it. Tens of megabytes cost a server nothing that
/// matters, and returning them costs `madvise` calls and the refaults of the next burst
/// (`docs/research/burst-start.md`). The same floor is what the default background thread's
/// start waits for, since below it the thread would have nothing to do, and what the
/// allowance for holding large slots starts from. `collect` (and `malloc_trim`) and growth
/// over the cap are not floored.
pub const ALLOWANCE_FLOOR_HUGE_PAGES: usize = 16;

// The floor is spent freely and the cap is what growth pays down to, so the floor must be the
// lower: a heap kept at the floor is never over the cap by what the floor keeps.
const _: () = assert!(ALLOWANCE_FLOOR_HUGE_PAGES < RETAINED_CAP_HUGE_PAGES);

/// Growth is charged against the cap in steps of this many huge pages: 1, or 2 MiB. Each step
/// of carving, while retained memory is over the cap, pays for at most one bounded purge
/// walk, so a heap whose free memory is all fragmentation pays one walk per step of growth.
/// A step costs at least a huge-page fault or 512 small ones, which dwarfs the walk.
pub const GROWTH_STEP_HUGE_PAGES: usize = 1;

// A grower that leaves the debt to the background thread wakes it once a step, when its add
// crosses a multiple of the step (`thread_word::crosses`), which takes a power of two.
const _: () = assert!(GROWTH_STEP_HUGE_PAGES.is_power_of_two());

/// While the background thread pays for growth, the most growth that may stay unpaid before
/// slow paths pay it again, growers and heartbeats a step each, in huge pages: 8, or 16 MiB.
/// The thread is woken when a step of growth comes due and pays it within its wake-up's
/// latency, so a debt this large means the thread has not run (starved, throttled, or busy
/// with a long round), and the bound on retained memory must not rest on the scheduler
/// (design: "Heartbeat"). It bounds retained memory to the cap and about this much more while
/// the thread lags.
pub const GROWTH_UNPAID_HUGE_PAGES: usize = 8;

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

/// The most bitmap words a fill for a magazine claims from before it hands out what it has:
/// 256. A claim takes what one word has free, from one slot to 64, at about a hundred cycles
/// when it is one; so a magazine of up to 254 objects, the largest before magazines grew to
/// 1,022 (`classes::MagazineSize::MAX`), is filled whole from any bitmap as before, and a
/// larger one from words this sparse comes back partly filled, after the work the smaller one
/// cost. The fill's cost per object is then what it was, and its worst single call no larger.
/// Dense words fill a whole magazine in sixteen claims.
pub const FILL_CLAIMS: usize = 256;

/// The most pages a purge returns in one call: 64, 256 KiB. Adjacent free pages go back
/// together, since a call costs about the same whatever it returns (about 2 us in a process
/// whose other threads are running, for its TLB shootdowns, against 0.05 us a page:
/// `docs/research/burst-start.md`); the bound keeps one call's claims, a few hundred bitmap
/// words for the smallest classes, from holding up the allocations that want them.
pub const PURGE_MAX_RUN_PAGES: u32 = 64;

/// The most bitmap words a run's looks read, and so about the most its purge claims at once:
/// 64. A claim holds its words through the `madvise`, and a fill that meets them moves on and
/// may carve instead; for the smallest classes, eight words to a page, this keeps a run to 8
/// pages.
pub const PURGE_MAX_RUN_WORDS: usize = 64;

/// The clocked pass returns a run of free pages only if it has at least this many: 4. A call
/// for one page costs four times as much a page as one for four, and a short run is the kind a
/// burst's next allocations land on. Growth over the cap and `collect` take any run. Runs of 8
/// saved few more calls and left twice the memory waiting after a peak
/// (`docs/research/burst-start.md`).
pub const PURGE_MIN_RUN_PAGES: u32 = 4;

/// One clocked pass in this many takes runs of any length: 8, so a single free page goes back
/// within about eight intervals of the program going quiet, and a long run within one.
pub const PURGE_ANY_RUN_EVERY: u64 = 8;

/// The intervals in which the program ran that a claim's frontier keeps the clocked pass off
/// the pages below it: 2, the interval of the claim and the next one in which a heartbeat ran
/// (`layout::FrontierFormat::floor`, whose test holds it to this). A silence counts as none
/// (`purge`).
pub const FRONTIER_INTERVALS: u64 = 2;

/// How much bitmap a heartbeat's step of a decay pass may read, in words: 256 words, 2 KiB.
/// Each word covers 64 slots, so this is one 4 KiB page of 8-byte slots read eight times over,
/// or a few hundred pages of larger ones. A step always finishes the unit it started, so a
/// huge-page unit of small slots may read more.
pub const PURGE_STEP_WORDS: usize = 256;

/// How many pages a heartbeat's step of a decay pass may purge, in 4 KiB pages: 128, half a
/// MiB, checked between units and within a unit that is one large slot, which a step returns
/// a piece at a time (`purge`). A heartbeat runs on a request's slow path, and at about
/// 0.15 us a page and a few microseconds a call (`docs/research/burst-start.md`) this is some
/// 20 to 30 us. Without it one step made 4 `madvise` calls over 7,685 pages, about 1 ms. In a
/// class of a huge page or more a piece is a whole huge page at least, 512 pages, so that a
/// slot kept with its huge-page advice is not left with one partly returned, which stays
/// charged whole (`region`, "Huge pages for grown blocks"); the hold's look and a growth
/// payment are raised the same way.
pub const PURGE_STEP_PAGES: u32 = 128;

/// How much bitmap one of `maintain`'s steps of the clocked pass may read, in words: 64, a
/// quarter of a heartbeat's step. `maintain`'s caller may be an async runtime's worker that
/// yields between steps, and a step should hold it for tens of microseconds; a heartbeat's
/// step is paid for by a slow path that already did far more.
pub const PURGE_MAINTAIN_STEP_WORDS: usize = 64;

/// How many pages one of `maintain`'s steps of the clocked pass may purge before it stops, in
/// 4 KiB pages: 128, half a MiB. A `madvise` costs about 0.4 us a call and 0.1 us a resident
/// page alone, and a few microseconds a call more with other threads to shoot down
/// (`docs/research/burst-start.md`). Checked between units, so a step that meets a unit
/// larger than this, one slot of a large class, purges that whole slot.
pub const PURGE_MAINTAIN_STEP_PAGES: u32 = 128;

/// How much bitmap a step of growth over the cap may read while it purges, in words: 16384,
/// 128 KiB. The bound keeps one call from paying for a whole heap; the clocked pass and later
/// steps finish the job.
pub const PURGE_CAP_WORDS: usize = 1 << 14;

/// A region of a class that gets huge pages keeps its first this many huge pages on small
/// pages and marks the rest of itself for huge pages when a carve first needs memory past
/// them: 8, or 16 MiB. The first touch of a huge page makes all of it resident, so the page at
/// the watermark costs up to one huge page that the program never asked for: at most an
/// eighth of the class. Below the threshold a class pays 4 KiB faults and dTLB misses, which
/// are all of the build phases' gap to mimalloc (`docs/research/memory.md`, "Build phases
/// against mimalloc"): 4 MiB would close much of it, but would take churn at 1M live objects
/// 5% over jemalloc's memory and still leave BTreeMap behind mimalloc, whose remaining lead
/// there is the drop. A simulated heap whose regions are smaller than four times this uses a
/// quarter of a region instead, and at least one huge page.
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

/// The most retained large-slot memory a grown block's slot may join instead of going back to
/// the OS: 8 huge pages, or 16 MiB. A block that moves leaves behind the slot it outgrew, and
/// a block advised for huge pages because it grew is freed whole; both used to be purged at
/// once, which is right for a block grown once and wrong for a buffer grown and freed again
/// and again, which then faulted all of its memory back on every repetition. Up to this much
/// they are kept, resident and counted, for the next growth to land on; past it they go back.
///
/// This is not a level on frees, which may not purge (`purge`): a plain free never purges,
/// whatever the count. A purge here is charged only to a block the program copied or wrote
/// in full to reach its size, so it costs at most about what the program already spent on
/// that block, and never falls on one that was allocated, barely touched and freed.
pub const KEPT_GROWN_HUGE_PAGES: usize = 8;

/// A zeroed large request of at least this many huge pages that is answered from a slot known
/// to be zero, so that nothing was cleared and none of its pages is resident, is marked
/// (`Back::mark_unwritten`), and its free looks at a sample of its pages to see whether the
/// tenant left it mostly unwritten: 8, or 16 MiB. Not a bound on risk, since both outcomes of
/// the look are ordinary frees, but the size at which the look's fixed cost, about 12k cycles
/// for the sample and the `madvise`, is under 3% of the life of a block whose tenant writes a
/// sixteenth of it (`docs/research/meili-peak.md`, "Evidence at the free").
pub const UNWRITTEN_FLOOR_HUGE_PAGES: usize = 8;

/// How many pages of a marked block's extent its free looks at, one `mincore` of one page each,
/// the middle page of each of this many equal parts: 16. Each costs about 650 cycles, present
/// or absent. Above the floor, 16 tell a block written in a sixteenth from one written in
/// full, which is all the rule asks.
pub const UNWRITTEN_SAMPLES: usize = 16;

/// A marked block whose free finds at most one sampled page in this many resident was left
/// mostly unwritten, and goes back to the OS whole: 16, so at most one of the sixteen. A page
/// faulted costs about thirteen times what clearing a resident page does, so a tenant that
/// writes more than about a thirteenth of its block is cheaper served by a slot kept resident
/// and cleared; at a sixteenth the next sparse tenant costs no more than the clearing did
/// (`docs/research/meili-peak.md`, "Evidence at the free").
pub const UNWRITTEN_RESIDENT_ONE_IN: usize = 16;

/// The floor of [`UNWRITTEN_FLOOR_HUGE_PAGES`] in bytes for a geometry.
#[inline(always)]
pub const fn unwritten_floor(geometry: Geometry) -> usize {
    UNWRITTEN_FLOOR_HUGE_PAGES * geometry.huge_page_size()
}

/// The budget of [`KEPT_GROWN_HUGE_PAGES`] in bytes for a geometry.
#[inline(always)]
pub const fn kept_grown(geometry: Geometry) -> usize {
    KEPT_GROWN_HUGE_PAGES * geometry.huge_page_size()
}

/// In the small-page part of a region, each commit makes at least this many pages writable
/// beyond what the carve that asked for it needs: 16, or 64 KiB.
pub const COMMIT_AHEAD_PAGES: usize = 16;

/// In the huge-page part, each commit makes writable whole huge pages, at least this many beyond
/// what the carve needs: 1, or 2 MiB, so growth faults a huge page at a time.
pub const COMMIT_AHEAD_HUGE_PAGES: usize = 1;

/// The most a commit makes writable beyond what its carve needs, in huge pages: 32, or 64 MiB.
/// Between the least above and this, a commit goes as far ahead as the region has committed
/// already, so a region that grows commits about twice as much at each step and the number
/// of `mprotect` calls grows with the logarithm of the region's use. Each call takes the
/// process's memory-map lock for writing and the region's mapping with it, which stops every
/// page fault in that region and every other change to the address space until it is done:
/// at 64 KiB a step a parallel program growing its heap made thousands of calls a second and
/// its threads queued behind them (`docs/research/memory.md`, "Faults and system calls in real
/// programs"). The bound keeps what is writable and never touched, which costs commit charge
/// under strict overcommit but no memory, to at most the region's use or 64 MiB.
pub const COMMIT_AHEAD_MAX_HUGE_PAGES: usize = 32;

/// How many pages a commit makes writable beyond what its carve needs, in a region with
/// `used` pages committed above its origin: `used`, within `least` and `most`.
#[inline(always)]
pub const fn commit_ahead(used: u32, least: u32, most: u32) -> u32 {
    if used < least {
        least
    } else if used > most {
        most
    } else {
        used
    }
}

/// A step of the hold's fine count (`hold::Fine`), in ticks of the cycle counter: 2^23,
/// about 2 ms. A reading of the counted clock steps the count by at most one, so across a
/// silence a hold ages by one step, not by the silence.
pub const FINE_TICKS: u64 = 1 << 23;

/// How many fine steps a large-class region must go without a free that leaves a slot resident
/// before its free slots may go back to the OS: 64, about 125 ms for a program that beats at
/// least once a fine step (`hold`). Long enough for most of the reuse that
/// follows a free (86 to 100% of the large slots taken again after a purge were taken within
/// 60 ms in tpchgen, gix-hours and datafusion, `docs/research/memory.md`, "Faults and system
/// calls in real programs"); short next to the clocked pass's second, which let DataFusion
/// hold every query's free large slots on top of the next query's ("DataFusion's peak against
/// jemalloc's"). On DataFusion 125 ms held the peak lower than 60 ms (842 against 848 MiB)
/// with fewer faults (+14% against +19% over no hold); 250 ms let it back up to 1,075 MiB, as
/// its queries' free blocks outlived the hold of the next (`docs/research/large-hold.md`).
pub const HOLD_STEPS: u32 = 64;

/// The most a heartbeat's look at the hold returns, in huge pages: 1, or 2 MiB, checked between
/// units. A large class's unit is at most a slot, so a look that meets a slot larger than the
/// rest of this returns that whole slot: DataFusion's longest look, about a millisecond,
/// returned one slot of 34 MiB (`docs/research/large-hold.md`). The look runs on whichever slow
/// path took the heartbeat, so this bounds what a request pays for it, as a step of the clocked
/// pass bounds its walk; a look that stops here with more over the floor has the next heartbeat
/// carry on. The growth step and a moving block's piece return the same at a time.
pub const HOLD_BEAT_HUGE_PAGES: usize = 1;

/// The bound of [`HOLD_BEAT_HUGE_PAGES`] in bytes for a geometry.
#[inline(always)]
pub const fn hold_beat(geometry: Geometry) -> usize {
    HOLD_BEAT_HUGE_PAGES * geometry.huge_page_size()
}

/// The retained-memory cap in bytes for a geometry.
#[inline(always)]
pub const fn retained_cap(geometry: Geometry) -> usize {
    RETAINED_CAP_HUGE_PAGES * geometry.huge_page_size()
}

/// The floor of the allowance in bytes for a geometry (`ALLOWANCE_FLOOR_HUGE_PAGES`).
#[inline(always)]
pub const fn allowance_floor(geometry: Geometry) -> usize {
    ALLOWANCE_FLOOR_HUGE_PAGES * geometry.huge_page_size()
}

/// The growth step in bytes for a geometry.
#[inline(always)]
pub const fn growth_step(geometry: Geometry) -> usize {
    GROWTH_STEP_HUGE_PAGES * geometry.huge_page_size()
}

/// The most growth that may stay unpaid while the background thread pays for it
/// ([`GROWTH_UNPAID_HUGE_PAGES`]), in bytes for a geometry.
#[inline(always)]
pub const fn growth_unpaid(geometry: Geometry) -> usize {
    GROWTH_UNPAID_HUGE_PAGES * geometry.huge_page_size()
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
    // At least one, so that a class in a region of few huge pages still starts on small pages.
    let quarter = region_huge_pages / 4;
    let quarter = if quarter == 0 { 1 } else { quarter };
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
