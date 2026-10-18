# Safety ledger: `crates/asmalloc/src/back/region.rs`

## BACK-REGION-01: `host.purge(...)` in `purge_unit`

**Preconditions** (`Host::purge`, which for the heap is `Space::purge`). No thread reads or
writes the range during the call, and nothing in it is needed afterwards.

**The range.** Pages `unit.start..unit.end` of the region. A unit comes from one of two
places.

- The walk: `Layout::unit_below`. Kani's `purge_steps_move_down_within_one_part` proves
  `unit.end` is at most `carved_pages(carved)`: the pages lie wholly below the watermark as
  read before the step.
- A large free over its cap (`Back::purge_slot`): `Layout::slot_unit(slot)`, the pages wholly
  inside one slot. The slot is below the watermark: `Back::free` reaches `purge_slot` only
  after `release_all` validated the pointer as a handed-out slot (`validate` aborts unless
  `slot < carved`). Kani's `a_slot_unit_touches_only_its_slot_on_the_tiny_heap` proves the
  unit lies inside the slot, ends at or below `carved_pages(slot + 1)`, and touches no slot
  but that one, for every class and slot of the tiny heap; `Divisor::divide` being exact for
  every input carries the arithmetic to the other geometries, and the unit test
  `freed_large_slots_have_a_small_cap_of_their_own` runs it in the native one.

The watermark only grows: a carve's compare-and-swap replaces `carved` with a larger value,
and nothing else writes it. `region::top_up`, which the walk calls to bring the watermark to
the end of its huge page, moves it only by calling `carve`, and owns the slots it gets like
any other caller until it releases them. So pages below the watermark stay below it, and no
carve can hand out a slot that touches them. The scenario `a_carve_racing_a_purge` explores
this under interleaving.

**Proof.**

1. *Every slot that touches the range is held by this thread.* `touching(unit)` gives slots
   `first..=last`, defined as `(start / size, (end - 1) / size)`; `Divisor::divide` is proved
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

**Changed 2026-10-17, awaiting review.** `purge_unit` itself is unchanged. It has a second
caller (`Back::purge_slot`, with a unit from `Layout::slot_unit`), and the walk now calls
`region::top_up` before its first unit in a region. "The range" above was rewritten for
both. A reviewer should check: that a slot unit can only be reached for a slot below the
watermark; that `top_up`'s release of slots it carved cannot set a bit that is already set
(it releases exactly the range `carve` returned, which no one else has seen); and that a
slot freed, purged by its own free and freed again is covered by the double-free argument in
step 1.

**Review.** 2026-10-17, fresh adversarial reviewer: the protocol was accepted; three gaps were
found in the argument: exclusivity under a double free, the huge-page part not covered by the
cited proof, and no explored test of a carve or a second purger racing a purge. All three are
addressed above; the same reviewer checked the fixes and accepted them.
