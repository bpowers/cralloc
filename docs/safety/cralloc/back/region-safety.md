# Safety ledger: `crates/cralloc/src/back/region.rs`

Every entry here takes the `Host` it is given at its word: its layout is the layout of the
class the region belongs to. For the heap that is the `ClassHost` invariant in
`host-safety.md`; a host built for another class's region makes every range and slot number
below wrong.

## BACK-REGION-01: `host.purge(...)` in `purge_unit_as`

**Preconditions** (`Host::purge`, which for the heap is `Space::purge`). No thread reads or
writes the range during the call, and nothing in it is needed afterwards.

**The range.** Pages `unit.start..unit.end` of the region. `purge_unit` is `purge_unit_as`
trusting the clean bits and asking for no split; the callers of the two are the walk in
`back/purge.rs` (`purge_region` and `tail_step`) and the hold's walk (below). The walk's
range is a run of units from
`Layout::unit_below`, which it builds in
`run_below` by starting at one unit and extending it down by the next unit `unit_below` gives
while that unit ends where the run starts. Every page of the run is then a page of some unit
`unit_below` returned for the same watermark reading, so the run, like each of its units, lies
wholly below the watermark as read before the step: its end is the first unit's end, which is
at most `carved_pages(carved)`. A run of one unit is the unit itself, the case the evidence
below was written for; a longer run is a union of such units. What supports that, and how
far each piece of evidence reaches:

- Kani's `purge_steps_move_down_within_one_part` (in `verify_slow.rs`, so on demand, not in
  the gate) proves it for classes whose size is a power of two, which it assumes, in the
  geometries `any_used_plan` gives: x86-64's and the simulator's shapes.
- Kani's `units_touch_exactly_their_slots_on_the_tiny_heap` (also on demand) asserts
  `last < carved` for every class that fits the tiny heap's regions, power of two or not.
  No large class fits there.
- The unit test `purge_passes_tile_the_carved_pages` checks the units against direct
  arithmetic for every class that fits a region, large ones included, in the standard, native
  and tiny geometries, and `scripts/sim-pages` runs it on pages of 16 and 64 KiB too.
- For the real kernels' geometries and sizes that are not powers of two the argument is on
  paper: `unit_below` clamps `end` to `carved_pages(carved)`, which is
  `floor(carved * size / page)`, and only ever moves `end` down from there. It is arithmetic on
  the page's shift, the region's and the class, whatever they are.

The walk's other ranges are parts of such units, so they lie below the watermark too, and
`touching` is defined for any range of pages:

- a piece of a unit that is one large slot (`purge::piece` of the unit below its clean top);
- in `tail_step`, a whole huge page `start..end` with `end` a multiple of the huge page and
  at most `carved_pages(carved)` (the cursor is capped there), and `start` the huge page's
  first page or the class's origin page if that is higher;
- in `tail_step`, a run of units of `unit_below(carved, None, ..)`, the first cut to start no
  lower than the huge page, the run extended no lower than it.

No range starts below the class's origin page (`unit_below` and `tail_step` both floor there),
and the origin is a whole number of pages (`Plan::slot_origin`).

The watermark only grows. Once a region is named, only a carve's compare-and-swap writes its
`Grow` word, which raises `carved` or, publishing a commit, keeps it. Before the naming the
thread that holds the tag writes the word of a large class's region (`Back::assign`,
`Back::unprepare`), with nothing carved, and no walk has the region: the walks find regions by
their published tags (`host-safety.md`). `region::top_up`, which the walk calls to bring the
watermark to the end of its huge page, moves it only by calling `carve`, and owns the slots it gets like
any other caller until it releases them. So pages below the watermark stay below it, and no
carve can hand out a slot that touches them. The scenario `a_carve_racing_a_purge` explores
this under interleaving.

**Proof.**

1. *Every slot that touches the range is held by this thread.* `touching(unit)` gives slots
   `first..=last`, defined as `(start / size, (end - 1) / size)` for any range of pages, a run
   as much as a unit; `Divisor::divide` is exact for every input (`classes::verify`, and a test
   of every input in `classes::divisor`), so these are exactly the slots that overlap the pages. Kani proves it outright on the tiny heap's regions
   (`units_touch_exactly_their_slots_on_the_tiny_heap`, every class, watermark, cursor and
   huge-page part), and the unit test `purge_passes_tile_the_carved_pages` checks it against
   direct arithmetic in the standard and native geometries, huge-page parts included.
   Before the call, `purge_unit` claimed every one of them with `tree::ExactRun::claim`, word by
   word, each a compare-and-swap that succeeds only if all the word's bits for those slots are set
   (free) and clears them. If any claim failed it released what it
   had and returned before the call. A cleared bit is held by exactly the thread whose
   compare-and-swap cleared it, until that thread sets it again: every other path that clears
   bits (`tree::claim`, `tree::claim_next`, `tree::ExactRun::claim`) is a compare-and-swap against
   a value with the bit set, and every path that sets one (`tree::release_one`, the ORs of a
   `tree::Release`) aborts if the bit was already set. Summary bits play no part: `ExactRun` clears
   those above the words it emptied, a group at a time, before the call, and they are hints.

   That last check does not make the claim exclusive by itself. If the program frees a slot,
   a purge claims it, and the program frees it again, the second `release` finds the bit clear
   and sets it without aborting; another thread may then allocate the slot and write it while
   the purge returns its page. The abort comes later, when the purge releases its claim.
   Exclusivity therefore rests on the program's contract (a slot is freed once per
   allocation), as every other guarantee about a double-freed slot does; the bitmap catches a
   double free when it arrives while the slot is free, not while a purge holds it.
2. *So no one uses the range.* A slot that is free in the bitmap is not in the program's hands
   and not in any magazine: slots leave the bitmap only by a claim (`fill`, `alloc`, a purge),
   and return only by `release`. The program cannot hold a slot this thread holds, unless it
   uses a slot after freeing it, which is outside the allocator's contract. The allocator keeps
   no state in object memory (`docs/design.md`, "Metadata never lives in freed memory"), so no
   allocator thread touches it either. Bytes of the range not in any slot do not exist: slots
   tile the region from the class's origin, a page boundary the range does not start below,
   and the range lies below the origin plus `carved * size`.
3. *Nothing in it is needed.* Free slots have no contents anyone reads. After the purge, the
   zero bits are set for slots wholly inside the range, which now read as zero. A slot that
   straddles an edge (the first or the last, both claimed, as is a large slot of which the range
   is a piece) has its zero bit set only if it is whole pages and every one of them is marked
   clean, and a clean page reads as zero (below). The bit is written before the release, under
   the claim. Without a split (`split` false) or with one, and whatever `trust` is, the bits are
   written only after `host.purge` returned true; `Purge::Clean` and `Purge::Unsplit` write
   none.

**A clean page reads as zero.** The zero bit of a slot returned in pieces rests on this, and the
next tenant of such a slot skips clearing it.

- *Where a bit is set.* Over pages just returned whole, by the holder of every slot that touches
  them (this purge, `mark_purged`, `mark_clean` after `purge_part` said the whole range went
  back), and over the pages of a slot just carved that lie outside its block (`mark_fresh`), which
  nothing has written. A refused or partial call sets none.
- *What a tenant writes.* A block of `size` bytes starts `into` bytes into its slot, at the slot's
  start or at its stagger, and its tenant may write to the end of the page its size ends in
  (`usable`). That is the pages from `first_page(slot, unit, into)` to below
  `extent_of(layout, slot, unit, size, _)`. The extent counts from the stagger whichever start the
  block has, so it is at or past the block's own end.
- *Where a bit is cleared.* Under those pages, by the holder, before the tenant has them:
  `settle_slot` at a claim and `cover` after a size is recorded (`resize_in_place`, `resize`) or
  advice given or kept (`grown_into`, a claim of a slot that kept it), each with the block's real
  `into`. A carve clears nothing, since the pages `mark_fresh` leaves alone are clear. Every
  in-place grow, from Rust or from C, goes through `resize_in_place`, so a tenant's pages widen
  only there.
- *Pages below the block.* A block at its stagger writes none of them, and they stay as they were,
  clean only if that is true. A block at its slot's start has its first page at the slot's: it
  clears them at its claim, a shrink may set them over pages it returned, and a grow back over them
  clears them again.
- *The allocator's own writes.* A move writes `to` only within the size `to` was claimed for
  (`copy_window`, BACK-CLASS-03). `unspill` writes inside the new size, in the call that then runs
  `cover`, with the slot held throughout, so nothing acts on the bits in between.
- *Huge faults.* A clean page under huge-page advice can become resident, with zeros, so it still
  reads as zero. `cover` clears from the slot's start in an advised slot for the count's sake.

`charge` and the walk's look (`all_clean`) read the same bits, and there a wrong one would cost
only a count or a purge. The random runs of `large_charge` check all this after every step, on the
native shape, which staggers by lines alone. Page staggers have the tests of
`back::tests::kernels` and `heap::tests::kernels`, each for one history.

The interleaving explorer checks the claim directly: `TWord` records which thread holds each
cleared leaf bit, and `TestHost::purge` fails the schedule unless the purging thread holds every
slot touching the range (`back::models::purge_*`, following `tla/Purge.tla`'s `PurgeSafe`). A
version of `purge_unit` that claims only the slots wholly inside fails those whose slots straddle
a unit, as `Purge_claim_inside.cfg` predicts. The scenarios include the huge-page part
(`purge_in_huge_pages`) and two purgers on the same units (`two_purgers_on_the_same_units`).

**The other caller: the hold's walk by slot ages** (`hold::walk_aged`). It calls `purge_unit` on
the pages of one free slot, `Layout::slot_unit(slot)`, or on a piece of them: `purge::piece` of
`Unit { end: top, ..unit }`, where `top` is the unit's clean top (`region::clean_top`, a page
of the unit), which gives pages of that unit and no others. The slot is below the watermark:
the walk visits slots below `upper`, which is the watermark read when the walk began
(`Grow::carved`, capped at the slot count) or a position an earlier look of the same walk
stored, itself below that, and the watermark only grows. The pages are whole pages of the
slot, so `touching` gives the slot alone and `purge_unit` claims it before the call. A slot a
heap hosts is whole pages because `heap::large::place` chooses only such classes: on a kernel with
large pages the large classes that are not go unused
(`a_large_block_gets_a_slot_of_whole_pages_on_every_kernel`). Kani's
`a_large_slot_is_whole_pages` does not reach those kernels. It covers x86-64's geometry and the
simulator's shapes, where every large class is whole pages. Steps 1 to 3 hold as written: they
rest on the claim `purge_unit` makes, not on how the unit was chosen. What the walk reads before (the leaf
word, the slot's size word and stamp, its clean bits) it reads with no claim, and it only
decides whether to try. The exploration `an_age_walk_races_large_tenants` runs the walk against
tenants that claim, trim, write, stamp and free the slots, and `TestHost::purge` fails any
schedule in which the purging thread does not hold every slot it returns.

## BACK-REGION-02: `host.purge(...)` in `purge_held`

**Preconditions** (`Host::purge`). No thread reads or writes the range during the call, and
nothing in it is needed afterwards.

**Proof.** `purge_held` is an `unsafe fn` whose caller promises that it holds the slot (the
slot's free bit is clear on the caller's behalf), that nothing uses the slot's memory during
the call or needs it after, and that the slot is below the watermark. The range is the pages
of `Layout::slot_unit(slot)`: `ceil(start / page)..floor(end / page)` of the slot's own
bytes, so every byte of it is inside the slot. With the slot held, no allocation can hand it
out, no purge walk can claim it, and the program has given it up, so the precondition holds
for the range. The two calls, both in a free's `purge_before_release` (the advised slot and
the block left unwritten), and why the free holds the slot are BACK-CLASS-01 in
`class-safety.md`, which also says what a double free can do.

`purge_held` declines a slot in a huge-page part (a purge there must be whole huge pages);
that is policy, not safety.

## BACK-REGION-04: `host.resident(...)` in `purge_unit_as`

**Preconditions** (`Host::resident`, which for the heap is `Space::resident`). No thread
writes the range during the call.

**Proof.** The call is made in `purge_unit_as` after the loop that claims every slot touching
the unit (`tree::ExactRun::claim` over `touching(unit)`) has succeeded and before any release: the
same point and the same range as BACK-REGION-01's purge, whose step 1 shows that this thread
then holds every slot touching the range, and that every byte of the range lies in one of
them. A slot's bytes are written only by its holder, so
nothing writes the range while the claims are held. The call reads nothing the allocator relies
on: on Linux it is `mincore`, and in the simulator it reads the range, which the claims keep
still. Its answer only decides whether the unit is purged; a wrong answer leaves resident
pages for a later pass or purges pages that were not resident, neither of which touches a held
slot. The tests `a_clean_run_that_is_not_resident_costs_one_question`,
`a_refused_question_falls_back_to_returning_the_clean_runs` and
`every_clean_run_of_a_rebuilt_huge_page_goes_back` run the call on the simulator natively, on
one thread; they are ignored under Miri, so no machine check looks for a write racing the call,
and the claim argument above is what rests on paper.

## BACK-REGION-03: `host.purge(...)` in `purge_part`

**Preconditions** (`Host::purge`). No thread reads or writes the range during the call, and
nothing in it is needed afterwards.

**Proof.** `purge_part` is an `unsafe fn` whose caller promises that it holds the slot, that
nothing uses the range during the call and that nothing in it is needed after. The function
checks that `from + len` is within the slot's size, so the range is inside the slot the
caller holds. It turns the range into pages by rounding both ends down, so a `from` that was
not a page multiple would take the page below it: every caller passes page multiples (the
function's documentation asks for them; BACK-CLASS-02, -03, -05 and -08 each say why theirs
are), and a large slot starts on a page boundary. When the slot is advised for huge pages it may then move the start up and the
end down to huge-page boundaries inside the slot (`Layout::huge_interior`), and returns
without a call if nothing is left: the range purged is a subrange of the one the caller gave
up, so the precondition still holds for it. It returns `true` only when the range it was
given went back whole, which BACK-CLASS-03 relies on before marking a slot zero. Its
callers and why their ranges are given up are BACK-CLASS-02 (the tail of an object that
shrank in place), BACK-CLASS-03 (the part of a moving object already copied),
BACK-CLASS-05 (`unspill`: a huge page of a block that grew in place, whose bytes are first
copied aside and copied back after) and BACK-CLASS-08 (`trim_slack`: the pages of a slot just
claimed that lie past both its new block and its last tenant) in `class-safety.md`.

Reviewed: 0bc95dc
