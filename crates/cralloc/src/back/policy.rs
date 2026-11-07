//! The back end's policy: thresholds, budgets and step sizes, all in one place.
//!
//! The heap's rules mean the same on every kernel, so each is stated in what it is about:
//!
//! - **A rule about memory** is bytes. So that a simulated heap keeps its scale, the bytes are
//!   counted in units its geometry gives: a *huge unit* ([`Geometry::huge_unit`]) and a *page
//!   unit* ([`Geometry::page_unit`]), 2 MiB and 4 KiB on every real kernel, and a simulated heap's
//!   own huge page and page. The names end `_HUGE_UNITS` and `_PAGE_UNITS`.
//! - **A bound on the kernel's work** is pages, whatever their size: the kernel's cost is per page
//!   (81 ns a 16 KiB page, 81 to 96 a 4 KiB page on the reference machine,
//!   `docs/research/machine-aarch64.md`, "What other calls charge"), so the same number of pages
//!   takes about the same time on every kernel. The names end `_PAGES`.
//! - **A rule about the kernel's huge page** is that page: what one costs when it is wrong is a
//!   fraction of what it holds. The names end `_HUGE_PAGES`. On a kernel whose huge page is over
//!   the retained cap, or larger than a region, these find none (`Geometry::in_regions_of`).
//!
//! The comment on each gives its value on a real kernel and why it has that value. The memory-side
//! values come from `docs/research/memory.md` and `docs/research/large-blocks.md`; none has been
//! tuned against speed yet.

use crate::space::Geometry;

/// The cap on retained memory, small and large classes together: 64 MiB. Retained memory is free
/// memory in the bitmaps that is believed resident. The cap is enforced when the heap grows, never
/// when it frees: a level cannot tell memory that is idle from memory about to be reused, and
/// purging on frees faulted reused memory straight back (`docs/research/memory.md`, "Purge on a
/// clock, never on a level"). Address space is segregated by class, so free memory of one class
/// serves another only through the kernel; the cap bounds what a phase change can leave resident
/// above what is live. The clocked pass returns memory in the ordinary case.
pub const RETAINED_CAP_HUGE_UNITS: usize = 32;

/// The floor of the allowance: 32 MiB. Memory up to this much is spent freely: the clocked pass
/// returns nothing while what a purge could return (by the last lap's count, `purge`) totals no
/// more than the floor, and stops at the floor when above it. Tens of megabytes cost a server
/// nothing that matters, and returning them costs `madvise` calls and the refaults of the next
/// burst (`docs/research/servers.md`, "What returning memory costs a request"). The same floor is
/// what the default background thread's start waits for, since below it the thread would have
/// nothing to do, and what the allowance for holding large slots starts from. `collect` (and
/// `malloc_trim`) and growth over the cap are not floored.
pub const ALLOWANCE_FLOOR_HUGE_UNITS: usize = 16;

// The floor is spent freely and the cap is what growth pays down to, so the floor must be the
// lower: a heap kept at the floor is never over the cap by what the floor keeps.
const _: () = assert!(ALLOWANCE_FLOOR_HUGE_UNITS < RETAINED_CAP_HUGE_UNITS);

/// Growth is charged against the cap in steps of this much: 2 MiB. Each step of carving, while
/// retained memory is over the cap, pays for at most one bounded purge walk, so a heap whose free
/// memory is all fragmentation pays one walk per step of growth. A step costs at least a huge-page
/// fault or a hundred small ones, which dwarfs the walk.
pub const GROWTH_STEP_HUGE_UNITS: usize = 1;

// A grower that leaves the debt to the background thread wakes it once a step, when its add
// crosses a multiple of the step (`thread_word::crosses`), which takes a power of two.
const _: () = assert!(GROWTH_STEP_HUGE_UNITS.is_power_of_two());

/// While the background thread pays for growth, the most growth that may stay unpaid before
/// slow paths pay it again, growers and heartbeats a step each: 16 MiB. The thread is woken when a
/// step of growth comes due and pays it within its wake-up's latency, so a debt this large means
/// the thread has not run (starved, throttled, or busy with a long round), and the bound on
/// retained memory must not rest on the scheduler (design: "The background thread"). It bounds
/// retained memory to the cap and about this much more while the thread lags.
pub const GROWTH_UNPAID_HUGE_UNITS: usize = 8;

/// The decay interval in ticks of [`crate::os::Os::ticks`]: 2^32, about a second of the cycle
/// counter at a few gigahertz, the same as the idle check's. A pass over the heap starts at
/// most once an interval. A second is long next to the microseconds between heartbeats and
/// short next to the ten seconds over which jemalloc returns everything.
pub const DECAY_TICKS: u64 = 1 << 32;

/// The most bitmap words a fill for a magazine claims from before it hands out what it has:
/// 256. A claim takes what one word has free, from one slot to 64, at about a hundred cycles
/// when it is one; so a magazine of up to 254 objects, the largest before magazines grew to
/// 1,022 (`classes::MagazineSize::MAX`), is filled whole from any bitmap as before, and a
/// larger one from words this sparse comes back partly filled, after the work the smaller one
/// cost. The fill's cost per object is then what it was, and its worst single call no larger.
/// Dense words fill a whole magazine in sixteen claims.
pub const FILL_CLAIMS: usize = 256;

/// The most pages a purge returns in one call: 64, 256 KiB with pages of 4 KiB. Adjacent free
/// pages go back together, since a call costs about the same whatever it returns (about 2 us in a
/// process whose other threads are running, for its TLB shootdowns, against 0.05 us a page:
/// `docs/research/servers.md`, "What returning memory costs a request"); the bound keeps one
/// call's claims, a few hundred bitmap words for the smallest classes, from holding up the
/// allocations that want them.
pub const PURGE_MAX_RUN_PAGES: u32 = 64;

/// The most bitmap words a run's looks read, and so about the most its purge claims at once:
/// 64. A claim holds its words through the `madvise`, and a fill that meets them moves on and
/// may carve instead; for the smallest classes, eight words to a 4 KiB page, this keeps a run to 8
/// such pages.
pub const PURGE_MAX_RUN_WORDS: usize = 64;

/// The clocked pass returns a run of free pages only if it holds at least this much: 16 KiB. A
/// call for 4 KiB costs four times as much a byte as one for 16, and a short run is the kind a
/// burst's next allocations land on. Growth over the cap and `collect` take any run. Runs of
/// 32 KiB left twice the memory waiting after a peak (`docs/research/servers.md`, "What returning
/// memory costs a request").
pub const PURGE_MIN_RUN_PAGE_UNITS: u32 = 4;

/// The shortest run of free pages the clocked pass returns, in pages for a geometry: at least one,
/// where a page is all of [`PURGE_MIN_RUN_PAGE_UNITS`] or more.
#[inline(always)]
pub const fn purge_min_run(geometry: Geometry) -> u32 {
    (PURGE_MIN_RUN_PAGE_UNITS as usize * geometry.page_unit()).div_ceil(geometry.page_size()) as u32
}

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

/// How many pages a heartbeat's step of a decay pass may purge: 128, half a MiB with pages of
/// 4 KiB, checked between units and within a unit that is one large slot, which a step returns
/// a piece at a time (`purge`). A heartbeat runs on a request's slow path, and at about
/// 0.15 us a page and a few microseconds a call (`docs/research/servers.md`, "What returning
/// memory costs a request") this is some 20 to 30 us. Without it one step made 4 `madvise`
/// calls over 7,685 pages, about 1 ms. Where a huge page is a huge unit, 2 MiB, a piece in a class
/// of a huge page or more is a whole huge page at least, so that a slot kept with its huge-page
/// advice is not left with one partly returned, which stays charged whole (`region`, "Huge pages
/// for grown blocks"; [`whole_huge_pieces`]); the hold's look and a growth payment are raised the
/// same way.
pub const PURGE_STEP_PAGES: u32 = 128;

/// How much bitmap one of `maintain`'s steps of the clocked pass may read, in words: 64, a
/// quarter of a heartbeat's step. `maintain`'s caller may be an async runtime's worker that
/// yields between steps, and a step should hold it for tens of microseconds; a heartbeat's
/// step is paid for by a slow path that already did far more.
pub const PURGE_MAINTAIN_STEP_WORDS: usize = 64;

/// How many pages one of `maintain`'s steps of the clocked pass may purge before it stops: 128,
/// half a MiB with pages of 4 KiB. A `madvise` costs about 0.4 us a call and 0.1 us a resident
/// page alone, and a few microseconds a call more with other threads to shoot down
/// (`docs/research/servers.md`, "What returning memory costs a request"). Checked between units
/// and within a unit that is one large slot, which a step returns a piece at a time (`purge`).
pub const PURGE_MAINTAIN_STEP_PAGES: u32 = 128;

/// Whether a step that returns a free slot of `size` bytes a piece at a time cuts its pieces in
/// whole huge pages. Only where a huge page is a huge unit: a slot whose advice the kernel refused
/// to take back is then not left with a huge page partly returned, and such a piece is about a
/// step's worth of pages. A piece of a larger huge page would be thousands of pages, a step many
/// times over, to protect against a refusal that is rare and costs only a charge until reclaim.
#[inline(always)]
pub const fn whole_huge_pieces(geometry: Geometry, size: usize) -> bool {
    geometry.huge_page_size() <= geometry.huge_unit() && size >= geometry.huge_page_size()
}

/// How much bitmap a step of growth over the cap may read while it purges, in words: 16384,
/// 128 KiB. The bound keeps one call from paying for a whole heap; the clocked pass and later
/// steps finish the job.
pub const PURGE_CAP_WORDS: usize = 1 << 14;

/// A region of a class that gets huge pages keeps its first this many huge pages on small
/// pages and marks the rest of itself for huge pages when a carve first needs memory past
/// them: 8, or 16 MiB with huge pages of 2 MiB and 256 MiB with 32 MiB. The first touch of a huge
/// page makes all of it resident, so the page at the watermark costs up to one huge page that the
/// program never asked for: at most an eighth of the class, whatever the huge page's size. Below
/// the threshold a class pays small faults and dTLB misses, which are most of the build phases'
/// gap to mimalloc (`docs/research/program-effects.md`, "TLB and huge pages"): 4 MiB of 2 MiB
/// pages would close much of it, but would take churn at 1M live objects 5% over jemalloc's memory
/// (`docs/research/memory.md`, "The huge-page threshold") and still leave BTreeMap behind
/// mimalloc, whose remaining lead there is the drop. A heap whose regions are smaller than four
/// times this uses a quarter of a region instead, and at least one huge page.
pub const HUGE_AFTER_HUGE_PAGES: usize = 8;

/// When claims reach above a region's tail (`tail`), the advice goes back on up to the
/// frontier and this much beyond it: 16 MiB, and at least one huge page. With none, a class that
/// grows back into its tail moved the boundary about ten times an interval, a call each that takes
/// the address-space lock; 16 MiB cut that five times and 64 MiB seven times, with no change in
/// memory. It is bounded because under the advice khugepaged rebuilds any huge page with one page
/// in use, so advice over units that stay sparse brings their memory back
/// (`docs/research/memory.md`, "Returning part of a huge page frees nothing until it is split",
/// "The tail rule, and what it cost").
pub const TAIL_HEADROOM_HUGE_UNITS: usize = 8;

/// [`TAIL_HEADROOM_HUGE_UNITS`] in huge pages for a geometry.
#[inline(always)]
pub const fn tail_headroom(geometry: Geometry) -> u32 {
    (TAIL_HEADROOM_HUGE_UNITS * geometry.huge_unit()).div_ceil(geometry.huge_page_size()) as u32
}

/// The pages of a huge page above the frontier that the walk's cheap look samples before it
/// reads the huge page a page at a time: 8, one in each eighth of the huge page, so that a
/// look at a dense huge page reads a few words more than main's did (its leaf words to the
/// first live slot). Reading a dense huge page a page at a time cost the background thread
/// four times main's walk at the start of a server's burst (`docs/research/memory.md`,
/// "The tail rule, and what it cost"). A fixed count, not one scaled with the region or the
/// heap: it bounds the cost of a look at one huge page, and the walk's budget bounds how many
/// it looks at.
pub const TAIL_SAMPLE_PAGES: u32 = 8;

/// A huge page above the frontier is counted exactly (`tail::free_resident`), and so may be
/// split, only if at least this many of its sampled pages are free and resident: 2 of 8, as
/// the share [`TAIL_SPLIT_SHARE`] asks for. The sample only saves the count's reading on huge
/// pages that are plainly dense; every lap samples other pages (`tail::look`), so a huge page
/// misjudged once is judged again. Sampling runs of four pages, as long as the walk's
/// shortest, instead returned a third as much on the lifetime job, whose stranded huge pages
/// have free runs of four pages that are rarely aligned to four.
pub const TAIL_SPARSE_PAGES: u32 = 2;

/// A huge page above the frontier is split, and its free pages returned, only if at least one
/// page in this many is free and resident by the walk's own count: 4, a quarter of the huge
/// page, 512 KiB of 2 MiB. Below that, what would go back is not worth the huge page's mapping and
/// an advice change. A cut, not a rate: a huge page under it is never split, however many laps
/// sample it. `trim` takes everything, and is not held to it.
pub const TAIL_SPLIT_SHARE: u32 = 4;

/// Whether a step on a request's slow path may ask the kernel to split a huge page: only one that
/// is a huge unit, 2 MiB. A split of 32 MiB takes 1.0 to 1.8 ms (`docs/research/machine-aarch64.md`,
/// "Huge pages: worth having at 2 and 32 MiB, off at 512 MiB"), and a millisecond spent there once
/// a second is a server's tail. Larger ones are split by the background thread and `collect`, and
/// otherwise stay whole.
#[inline(always)]
pub const fn request_may_split(geometry: Geometry) -> bool {
    geometry.huge_page_size() <= geometry.huge_unit()
}

/// Only classes with at least this many slots to a page unit get huge pages: 4, so classes up
/// to 1 KiB. Small objects are dense in their pages, so a huge page holds what the program
/// uses. A block of a page or more is often touched in part, and with huge pages every part
/// touched would cost a huge page: 256 blocks of 1 MiB with one page written each were 246 MiB
/// resident with huge pages of 2 MiB. In bytes, not in the kernel's pages, or a page of 16 KiB
/// would take in classes of 4 KiB, which are not dense in the same way.
pub const HUGE_MIN_SLOTS_PER_PAGE_UNIT: usize = 4;

/// A large block that grows to at least this many huge pages, by moving or in place, gets huge
/// pages up to its new size, if the last piece the program wrote holds data in at least half
/// its pages: those of its slot's whole huge pages are advised for them
/// (`region::advise_grown`) until the slot is freed, when they are purged and the advice is
/// taken back. 1. Growth by appending is the evidence of dense use, and the bytes written last are
/// checked to be dense; the advice never covers the slot's room past the size asked for, so it
/// runs at most one growth step ahead of what has been shown. A block allocated at its size in one
/// call has shown neither, and with huge pages each part touched would cost a huge page. The
/// soak's growing buffers, written a stripe a megabyte past 64 MiB, were 1013 MiB resident with
/// the whole slot advised at each growth, and 271 without.
///
/// Half, because a block that holds data in at least half its pages costs at most twice what
/// it touches when huge pages back it; below that the cost can approach a whole huge page a
/// page. One huge page, because a smaller block has no whole huge page to advise, and from one up
/// the advice makes resident at most one huge page that the program did not write.
pub const GROWN_HUGE_MIN_HUGE_PAGES: usize = 1;

/// ...and only if it at most doubles at once: a doubling is the signature of appending. A
/// larger jump is a one-off reserve that says nothing about how densely the buffer will be
/// used, and khugepaged would make every huge page it touches resident whole
/// (`docs/research/hardware.md`, "Huge pages are bound by zeroing, and by free 2 MiB blocks"):
/// 64 buffers written 1 MiB densely, grown to 4 MiB at once and then written sparsely held
/// 160 MiB with huge pages against 77 without.
pub const GROWN_HUGE_MAX_FACTOR: usize = 2;

/// A large block whose alignment allows it starts this many bytes past its slot's start for
/// each step of its slot's index, over [`STAGGER_SLOTS`] slots, so that the same offset in large
/// blocks of one class falls in different cache sets. Slots are spaced by their class size, often
/// a power of two, and buffers walked together at slot starts compete for one set of every cache
/// level: 64 buffers of 4 MiB written round-robin took 7.9 ns an element at slot starts and 0.74
/// to 0.80 staggered (`docs/research/program-effects.md`, "Order, and lines and pages"). 64 bytes
/// on every target, though `lined::LINE` is 128 on aarch64: that is a bound for false sharing,
/// where this is about sets, and every L1 we know of indexes by 64.
pub const STAGGER_STEP: usize = 64;

/// The number of distinct line staggers: 64, so they span 4 KiB, which is every set of an L1 of 64
/// sets, 64 sets apart in the L2, and a quarter of the 256 sets of a Firestorm L1.
pub const STAGGER_SLOTS: usize = 64;

/// The room the line staggers take beyond a block's size: 4 KiB. It costs address space, and at
/// most one more page of memory where a page is at least this long.
pub const STAGGER_LINES: usize = STAGGER_STEP * STAGGER_SLOTS;

/// The most whole pages a block's stagger moves it over: 32. Lines alone leave buffers written in
/// turn on the same pages' numbers, which index the TLB's sets, when slots are spaced by a power
/// of two: 64 buffers in slots of 1,024 pages of 16 KiB took 7.5 ns an element with lines alone and
/// 0.46 with pages too (`docs/research/machine-aarch64.md`, "Blocks written in turn: the stagger
/// needs pages as well as lines").
pub const STAGGER_PAGES_MAX: u32 = 32;

/// The share of a slot the page term may take: an eighth. So the room costs a class at most one
/// step up, and a block is never moved so far that its own slot is mostly room.
pub const STAGGER_PAGES_SHARE: usize = 8;

/// Whether this build can stagger by pages at all. x86-64's geometry does not, and in its builds
/// this makes the arithmetic fold away; every build that may host a simulated kernel keeps it.
pub(crate) const PAGE_STAGGER: bool =
    cfg!(any(target_arch = "aarch64", test, kani, feature = "sim"));

/// How many whole pages a slot of `slot` bytes staggers over: a slot's own index modulo this,
/// `P`, is the page term (`Layout::stagger`). `min(32, slot_pages / 8)`, and none if that is under
/// two or the geometry does not stagger by pages. In pages, since the TLB's sets index page-number
/// bits: on pages of 4 KiB, 16 KiB and 64 KiB the first slot with a term is 64 KiB, 256 KiB and
/// 1 MiB. Being wrong costs little: at most an eighth of a slot's address space, and no memory.
#[inline(always)]
pub const fn stagger_pages(geometry: Geometry, slot: usize) -> u32 {
    if !PAGE_STAGGER || !geometry.staggers_pages() {
        return 0;
    }
    let share = slot / geometry.page_size() / STAGGER_PAGES_SHARE;
    if share < 2 {
        0
    } else if share > STAGGER_PAGES_MAX as usize {
        STAGGER_PAGES_MAX
    } else {
        share as u32
    }
}

/// The room a block needs in a slot of `slot` bytes for any stagger to fit: 4 KiB for the lines,
/// and the page term's pages. At most 132 KiB, 516 KiB or 2 MiB on pages of 4 KiB, 16 KiB or
/// 64 KiB; 4 KiB on x86-64. The stagger itself is less by a page and a line.
#[inline(always)]
pub const fn stagger_room(geometry: Geometry, slot: usize) -> usize {
    STAGGER_LINES + stagger_pages(geometry, slot) as usize * geometry.page_size()
}

/// A large block that moves is copied in pieces of this much, and the pages of the old block
/// behind each piece go back to the OS before the next is copied: 2 MiB. So a moving block has both
/// copies resident for at most one piece, where a single copy had them for its whole size (four
/// threads growing buffers to 256 MiB peaked at three times glibc, which moves by remapping). A
/// piece costs one `madvise` on top of its copy; 2 MiB copies in about a hundred microseconds, so
/// the call is a few percent, and a smaller piece would buy little memory for more calls. A block
/// of at most one piece is copied whole. A block under huge-page advice goes in whole huge pages,
/// since purging part of one returns nothing (`Back::move_large`).
pub const MOVE_PIECE_HUGE_UNITS: usize = 1;

/// The most that outgrown slots may hold, kept resident and not yet taken again: 64 MiB. A block
/// that moves because it outgrew its slot leaves that slot behind; the next buffer growing through
/// the class takes it within milliseconds, and a slot returned at once is faulted straight back
/// (tpchgen faulted back 2.3 GiB of them a run). So an outgrown slot is kept while the outgrown
/// slots kept so far, counted on their own (`State::outgrown`), are within this, and goes back
/// behind the copy past it.
///
/// The count is of those slots alone, not of free large memory, which the hold keeps above
/// its floor by design, so that any budget set against it was always spent on a large heap.
/// The bound is what a trail of outgrown slots may cost: where the moves land in slots the
/// heap reuses, growth pays nothing toward the cap, and 256 buffers grown on a 2 GiB heap left
/// 540 MiB behind them with no bound (`docs/research/large-blocks.md`, "What growth leaves
/// behind").
///
/// It equals the retained cap today because the two bound the same thing from two sides: the
/// cap is what free memory may reach while the heap grows by carving, and this is what a trail
/// may reach while it grows by reuse, which the cap does not see. They are separate constants
/// because they answer different evidence and need not move together.
///
/// A purge here falls only on a block the program copied to reach its size, so it costs at
/// most about what the program already spent on that block.
pub const KEPT_OUTGROWN_HUGE_UNITS: usize = 32;

/// A zeroed large request of at least this much that is answered from a slot known to be zero, so
/// that nothing was cleared and none of its pages is resident, is marked
/// (`Back::mark_unwritten`), and its free looks at a sample of its pages to see whether the
/// tenant left it mostly unwritten: 16 MiB. Not a bound on risk, since both outcomes of the look
/// are ordinary frees, but the size at which the look's fixed cost, about 12k TSC ticks for the
/// sample and the `madvise`, is under 3% of the life of a block whose tenant writes a sixteenth of
/// it (`docs/research/large-blocks.md`, "Zeroed blocks: evidence at the free").
pub const UNWRITTEN_FLOOR_HUGE_UNITS: usize = 8;

/// How many pages of a marked block's extent its free looks at, one `mincore` of one page each,
/// the middle page of each of this many equal parts: 16. Each costs about 650 TSC ticks, present
/// or absent. Above the floor, 16 tell a block written in a sixteenth from one written in
/// full, which is all the rule asks.
pub const UNWRITTEN_SAMPLES: usize = 16;

/// A marked block whose free finds at most one sampled page in this many resident was left
/// mostly unwritten, and goes back to the OS whole: 16, so at most one of the sixteen. A page
/// faulted costs about thirteen times what clearing a resident page does, so a tenant that
/// writes more than about a thirteenth of its block is cheaper served by a slot kept resident
/// and cleared; at a sixteenth the next sparse tenant costs no more than the clearing did
/// (`docs/research/large-blocks.md`, "Zeroed blocks: evidence at the free").
pub const UNWRITTEN_RESIDENT_ONE_IN: usize = 16;

/// The floor of [`UNWRITTEN_FLOOR_HUGE_UNITS`] in bytes for a geometry.
#[inline(always)]
pub const fn unwritten_floor(geometry: Geometry) -> usize {
    UNWRITTEN_FLOOR_HUGE_UNITS * geometry.huge_unit()
}

/// The bound of [`KEPT_OUTGROWN_HUGE_UNITS`] in bytes for a geometry.
#[inline(always)]
pub const fn kept_outgrown(geometry: Geometry) -> usize {
    KEPT_OUTGROWN_HUGE_UNITS * geometry.huge_unit()
}

/// In the small-page part of a region, each commit makes at least this much writable beyond what
/// the carve that asked for it needs: 64 KiB, and at least a page.
pub const COMMIT_AHEAD_PAGE_UNITS: usize = 16;

/// [`COMMIT_AHEAD_PAGE_UNITS`] in pages for a geometry.
#[inline(always)]
pub const fn commit_ahead_least(geometry: Geometry) -> u32 {
    (COMMIT_AHEAD_PAGE_UNITS * geometry.page_unit()).div_ceil(geometry.page_size()) as u32
}

/// In the huge-page part, each commit makes writable whole huge pages, at least this many beyond
/// what the carve needs: 1, so growth faults a huge page at a time.
pub const COMMIT_AHEAD_HUGE_PAGES: usize = 1;

/// The most a commit makes writable beyond what its carve needs: 64 MiB, which holds a huge page
/// wherever they are used, since none over the retained cap is. Between the least above and this,
/// a commit goes as far ahead as the region has committed already, so a region that grows commits
/// about twice as much at each step and the number of `mprotect` calls grows with the logarithm of
/// the region's use. Each call takes the process's memory-map lock for writing and the region's
/// mapping with it, which stops every page fault in that region and every other change to the
/// address space until it is done: at 64 KiB a step a parallel program growing its heap made
/// thousands of calls a second and its threads queued behind them (`docs/research/memory.md`,
/// "Commit calls grow with the logarithm of a region's use, up to 64 MiB"). The bound keeps what
/// is writable and never touched, which costs commit charge under strict overcommit but no
/// memory, to at most the region's use or 64 MiB.
pub const COMMIT_AHEAD_MAX_HUGE_UNITS: usize = 32;

/// [`COMMIT_AHEAD_MAX_HUGE_UNITS`] in pages for a geometry.
#[inline(always)]
pub const fn commit_ahead_most(geometry: Geometry) -> u32 {
    (COMMIT_AHEAD_MAX_HUGE_UNITS * geometry.huge_unit() / geometry.page_size()) as u32
}

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
/// 60 ms in tpchgen, gix-hours and datafusion, `docs/research/large-blocks.md`, "Free slots: a
/// hold, and why no hold serves everyone"); short next to the clocked pass's second, which let
/// DataFusion hold every query's free large slots on top of the next query's. On DataFusion 125
/// ms held the peak lower than 60 ms (842 against 848 MiB) with fewer faults (+14% against +19%
/// over no hold); 250 ms let it back up to 1,075 MiB, as its queries' free blocks outlived the
/// hold of the next (the same section).
pub const HOLD_STEPS: u32 = 64;

/// The most a heartbeat's look at the hold returns: the pages of a huge unit, checked between
/// units and within a slot, which a look returns a piece at a time: one look returning a whole
/// slot of 34 MiB took about a millisecond on DataFusion (`docs/research/large-blocks.md`,
/// "Free slots: a hold, and why no hold serves everyone"). The look runs on whichever slow
/// path took the heartbeat, so this bounds what a request pays for it, as a step of the clocked
/// pass bounds its walk; a look that stops here with more over the floor has the next heartbeat
/// carry on. So 512 pages on every real kernel, 2 MiB of 4 KiB and 8 MiB of 16 KiB: the bound is on
/// time, 41 to 49 us on the reference machine and 42 on a Firestorm core.
#[inline(always)]
pub const fn hold_beat(geometry: Geometry) -> usize {
    geometry.huge_unit() / geometry.page_unit() * geometry.page_size()
}

/// The retained-memory cap in bytes for a geometry.
#[inline(always)]
pub const fn retained_cap(geometry: Geometry) -> usize {
    RETAINED_CAP_HUGE_UNITS * geometry.huge_unit()
}

/// The floor of the allowance in bytes for a geometry (`ALLOWANCE_FLOOR_HUGE_UNITS`).
#[inline(always)]
pub const fn allowance_floor(geometry: Geometry) -> usize {
    ALLOWANCE_FLOOR_HUGE_UNITS * geometry.huge_unit()
}

/// The growth step in bytes for a geometry.
#[inline(always)]
pub const fn growth_step(geometry: Geometry) -> usize {
    GROWTH_STEP_HUGE_UNITS * geometry.huge_unit()
}

/// The most growth that may stay unpaid while the background thread pays for it
/// ([`GROWTH_UNPAID_HUGE_UNITS`]), in bytes for a geometry.
#[inline(always)]
pub const fn growth_unpaid(geometry: Geometry) -> usize {
    GROWTH_UNPAID_HUGE_UNITS * geometry.huge_unit()
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
    MOVE_PIECE_HUGE_UNITS * geometry.huge_unit()
}

/// The most a clocked pass may purge when the last lap found `returnable` bytes it could
/// return, against a floor of `floor`: everything over the floor. What the pass may take is
/// already only what the last two intervals did not reuse (the frontiers), so a share of it
/// held back only keeps memory the program has shown it does not need: with a quarter an
/// interval the lifetime job's 18-second troughs ended 0.4 GiB higher at 2 GiB, for a third
/// fewer faults that cost no measurable cycles (`docs/research/memory.md`, "How much a clocked
/// pass returns"). The retained count and the cap play no part: the count is mostly free slots
/// beside live ones, which no purge can return, and the cap is growth's to enforce.
#[inline(always)]
pub const fn decay_goal(returnable: usize, floor: usize) -> usize {
    returnable.saturating_sub(floor)
}

/// The most the refresh is raised (`refresh_raise`): 3 halvings, from one retiring full
/// magazine in `depot::REFRESH_EVERY` (32) to one in 4. One in 4 took the lifetime job's 2 GiB
/// trough down by 16% for about 1% of cycles; one in every took little more for 10%
/// (`docs/research/memory.md`, "Refresh on evidence").
pub const REFRESH_RAISE_MAX: u32 = 3;

/// How many halvings of the refresh interval the fragmentation of the small classes earns:
/// `fragmented` bytes of free slots on pages that also hold something, against `carved` bytes
/// the small classes have carved. Zero while the share is under 1 in 64, then one more for
/// each doubling, up to [`REFRESH_RAISE_MAX`] from 1 in 16. A heap in steady use keeps a
/// share well under the first step (0.6% on the steady cache and at the lifetime job's peaks)
/// and pays nothing; a heap whose short-lived objects have just died around long-lived ones
/// keeps a fifth (the lifetime job's troughs) and gets the most. The step at 1 in 16 is low
/// enough that the share the most buys (12% at that trough) keeps it there.
#[inline(always)]
pub const fn refresh_raise(fragmented: usize, carved: usize) -> u32 {
    if carved == 0 {
        return 0;
    }
    // One for each of 1 in 64, 1 in 32 and 1 in 16 that the share reaches.
    let mut raise = 0;
    while raise < REFRESH_RAISE_MAX && fragmented >= carved.div_ceil(64 >> raise) {
        raise += 1;
    }
    raise
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

/// Whether a class of `size`-byte slots gets huge pages in a plan's geometry: never where the plan
/// uses none.
#[inline(always)]
pub const fn wants_huge(size: usize, geometry: Geometry) -> bool {
    geometry.huge_pages_usable() && size * HUGE_MIN_SLOTS_PER_PAGE_UNIT <= geometry.page_unit()
}

/// Whether a large block growing from `old` to `new` bytes gets huge pages in a plan's geometry:
/// never where the plan uses none.
#[inline(always)]
pub const fn grown_huge(old: usize, new: usize, geometry: Geometry) -> bool {
    geometry.huge_pages_usable()
        && new > old
        && new >= GROWN_HUGE_MIN_HUGE_PAGES * geometry.huge_page_size()
        && old.saturating_mul(GROWN_HUGE_MAX_FACTOR) >= new
}

// What every rule comes to on x86-64, whose geometry is the design's: 4 KiB pages and 2 MiB huge
// pages, so that its behaviour cannot change by a rule's restatement.
const _: () = {
    const MIB: usize = 1 << 20;
    let g = Geometry::DESIGN;
    assert!(g.page_unit() == g.page_size() && g.page_unit() == 4 << 10);
    assert!(g.huge_unit() == g.huge_page_size() && g.huge_unit() == 2 * MIB);
    assert!(retained_cap(g) == 64 * MIB);
    assert!(kept_outgrown(g) == 64 * MIB);
    assert!(allowance_floor(g) == 32 * MIB);
    assert!(growth_step(g) == 2 * MIB);
    assert!(growth_unpaid(g) == 16 * MIB);
    assert!(unwritten_floor(g) == 16 * MIB);
    assert!(move_piece(g) == 2 * MIB);
    assert!(hold_beat(g) == 2 * MIB);
    assert!(commit_ahead_least(g) == 16);
    assert!(commit_ahead_most(g) == 16384);
    assert!(purge_min_run(g) == 4);
    assert!(tail_headroom(g) == 8);
    assert!(request_may_split(g));
    assert!(wants_huge(1 << 10, g) && !wants_huge((1 << 10) + 1, g));
    assert!(!grown_huge(MIB, 2 * MIB - 1, g) && grown_huge(MIB, 2 * MIB, g));
    assert!(whole_huge_pieces(g, 2 * MIB) && !whole_huge_pieces(g, 2 * MIB - 1));
    assert!(stagger_pages(g, 4 << 30) == 0);
    assert!(stagger_room(g, 4 << 30) == 4 << 10);
    assert!(STAGGER_STEP == 64);
};

// What a rule about memory comes to on every real kernel: the same bytes.
const _: () = {
    const MIB: usize = 1 << 20;
    let shifts = [12, 14, 16];
    let mut i = 0;
    while i < shifts.len() {
        let g = Geometry::of_kernel(shifts[i]);
        assert!(retained_cap(g) == 64 * MIB);
        assert!(kept_outgrown(g) == 64 * MIB);
        assert!(allowance_floor(g) == 32 * MIB);
        assert!(growth_step(g) == 2 * MIB);
        assert!(growth_unpaid(g) == 16 * MIB);
        assert!(unwritten_floor(g) == 16 * MIB);
        assert!(move_piece(g) == 2 * MIB);
        assert!(commit_ahead_most(g) as usize * g.page_size() == 64 * MIB);
        assert!(commit_ahead_least(g) as usize * g.page_size() >= 64 << 10);
        assert!(purge_min_run(g) as usize * g.page_size() >= 16 << 10);
        // A bound on the kernel's work is pages, whatever their size.
        assert!(hold_beat(g) == 512 * g.page_size());
        // Classes up to 1 KiB where huge pages are used.
        let plan = g.largest_plan().geometry();
        assert!(wants_huge(1 << 10, plan) == plan.huge_pages_usable());
        assert!(!wants_huge((1 << 10) + 1, plan));
        i += 1;
    }
};
