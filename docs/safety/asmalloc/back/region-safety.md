# Safety ledger: `crates/asmalloc/src/back/region.rs`

## BACK-REGION-01: `host.purge(...)` in `purge_unit`

**Preconditions** (`Host::purge`, which for the heap is `Space::purge`). No thread reads or
writes the range during the call, and nothing in it is needed afterwards.

**The range.** Pages `unit.start..unit.end` of the region: a run of units from
`Layout::unit_below`, which the walk in `back/purge.rs` (`purge_unit`'s only caller) builds in
`run_below` by starting at one unit and extending it down by the next unit `unit_below` gives
while that unit ends where the run starts. Every page of the run is then a page of some unit
`unit_below` returned for the same watermark reading, so the run, like each of its units, lies
wholly below the watermark as read before the step: its end is the first unit's end, which is
at most `carved_pages(carved)`. A run of one unit is the unit itself, the case the evidence
below was written for; a longer run is a union of such units. What supports that, and how
far each piece of evidence reaches:

- Kani's `purge_steps_move_down_within_one_part` (in `verify_slow.rs`, so on demand, not in
  the gate) proves it for classes whose size is a power of two, which it assumes.
- Kani's `units_touch_exactly_their_slots_on_the_tiny_heap` (also on demand) asserts
  `last < carved` for every class that fits the tiny heap's regions, power of two or not.
  No large class fits there.
- The unit test `purge_passes_tile_the_carved_pages` checks the units against direct
  arithmetic for every class that fits a region, large ones included, in the standard and native
  geometries.
- For the design's geometry and sizes that are not powers of two the argument is on paper:
  `unit_below` clamps `end` to `carved_pages(carved)`, which is `floor(carved * size / page)`,
  and only ever moves `end` down from there.

The watermark only grows: a carve's compare-and-swap replaces `carved` with a larger value,
and nothing else writes it. `region::top_up`, which the walk calls to bring the watermark to
the end of its huge page, moves it only by calling `carve`, and owns the slots it gets like
any other caller until it releases them. So pages below the watermark stay below it, and no
carve can hand out a slot that touches them. The scenario `a_carve_racing_a_purge` explores
this under interleaving.

**Proof.**

1. *Every slot that touches the range is held by this thread.* `touching(unit)` gives slots
   `first..=last`, defined as `(start / size, (end - 1) / size)` for any range of pages, a run
   as much as a unit; `Divisor::divide` is proved
   exact for every input (`classes::verify`, `classes::verify_slow`), so these are exactly the
   slots that overlap the pages. Kani proves it outright on the tiny heap's regions
   (`units_touch_exactly_their_slots_on_the_tiny_heap`, every class, watermark, cursor and
   huge-page part), and the unit test `purge_passes_tile_the_carved_pages` checks it against
   direct arithmetic in the standard and native geometries, huge-page parts included.
   Before the call, `purge_unit` claimed every one of them with `claim_exact`, word by word,
   each a compare-and-swap that succeeds only if all the word's bits for those slots are set
   (free) and clears them. If any claim failed it released what it
   had and returned before the call. A cleared bit is held by exactly the thread whose
   compare-and-swap cleared it, until that thread sets it again: every other path that clears
   bits (`tree::claim`, `claim_exact`) is a compare-and-swap against a value with the bit set,
   and every path that sets one (`tree::release`) aborts if the bit was already set.

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
   tile the region from offset 0, and the range lies below `carved * size`.
3. *Nothing in it is needed.* Free slots have no contents anyone reads. After the purge, the
   zero bits are set only for slots wholly inside the range, which now read as zero; slots that
   straddle its edges keep their zero bits clear.

The interleaving explorer checks the claim directly: `TWord` records which thread holds each
cleared leaf bit, and `TestHost::purge` fails the schedule unless the purging thread holds every
slot touching the range (`back::models::purge_*`, following `tla/Purge.tla`'s `PurgeSafe`). A
version of `purge_unit` that claims only the slots wholly inside fails three of those
scenarios, as `Purge_claim_inside.cfg` predicts. The scenarios include the huge-page part
(`purge_in_huge_pages`) and two purgers on the same units (`two_purgers_on_the_same_units`).

**Changed 2026-10-17.** `purge_unit` is unchanged. The walk now calls `region::top_up` before
its first unit in a region, and "The range" says what that does to the watermark and how far
each proof of the range reaches. A first version of the large-class cap gave `purge_unit` a
second caller that claimed a freed slot back; the reviewer found that it widened the window
in which a double free is silent, and it is gone: a large free now purges while it still
holds the slot, under BACK-REGION-02 and `class-safety.md`.

## BACK-REGION-02: `host.purge(...)` in `purge_held`

**Preconditions** (`Host::purge`). No thread reads or writes the range during the call, and
nothing in it is needed afterwards.

**Proof.** `purge_held` is an `unsafe fn` whose caller promises that it holds the slot (the
slot's free bit is clear on the caller's behalf), that nothing uses the slot's memory during
the call or needs it after, and that the slot is below the watermark. The range is the pages
of `Layout::slot_unit(slot)`: `ceil(start / page)..floor(end / page)` of the slot's own
bytes, so every byte of it is inside the slot. With the slot held, no allocation can hand it
out, no purge walk can claim it, and the program has given it up, so the precondition holds
for the range. The one caller and why it holds the slot are BACK-CLASS-01 in
`class-safety.md`, which also says what a double free can do.

`purge_held` declines a slot in a huge-page part (a purge there must be whole huge pages);
that is policy, not safety. Accepted by the reviewer, 2026-10-17.

## BACK-REGION-03: `host.purge(...)` in `purge_part`

**Preconditions** (`Host::purge`). No thread reads or writes the range during the call, and
nothing in it is needed afterwards.

**Proof.** `purge_part` is an `unsafe fn` whose caller promises that it holds the slot, that
nothing uses the range during the call and that nothing in it is needed after. The function
checks that `from + len` is within the slot's size, so the range is inside the slot the
caller holds. When the slot is advised for huge pages it may then move the start up and the
end down to huge-page boundaries inside the slot (`Layout::huge_interior`), and returns
without a call if nothing is left: the range purged is a subrange of the one the caller gave
up, so the precondition still holds for it. It returns `true` only when the range it was
given went back whole, which BACK-CLASS-03 relies on before marking a slot zero. Its
callers and why their ranges are given up are BACK-CLASS-02 (the tail of an object that
shrank in place), BACK-CLASS-03 (the part of a moving object already copied) and
BACK-CLASS-05 (`unspill`: a huge page of a block that grew in place, whose bytes are first
copied aside and copied back after) in `class-safety.md`.

**Review.** 2026-10-17, fresh adversarial reviewer: the protocol was accepted; three gaps were
found in the argument: exclusivity under a double free, the huge-page part not covered by the
cited proof, and no explored test of a carve or a second purger racing a purge. All three are
addressed above; the same reviewer checked the fixes and accepted them.

Reviewed: 415b07e
