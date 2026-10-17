# Safety ledger: `crates/asmalloc/src/back/region.rs`

## BACK-REGION-01: `host.purge(...)` in `purge_unit`

**Preconditions** (`Host::purge`, which for the heap is `Space::purge`). No thread reads or
writes the range during the call, and nothing in it is needed afterwards.

**The range.** Pages `unit.start..unit.end` of the region, from `Layout::unit_below`. Kani's
`purge_steps_move_down_within_one_part` proves `unit.end` is at most `carved_pages(carved)`:
the pages lie wholly below the watermark as read before the step. The watermark only grows (a
carve's compare-and-swap replaces `carved` with a larger value, and nothing else writes it), so
the pages stay below it, and no carve can hand out a slot that touches them. The scenario
`a_carve_racing_a_purge` explores this under interleaving.

**Proof.**

1. *Every slot that touches the range is held by this thread.* `touching(unit)` gives slots
   `first..=last`, defined as `(start / size, (end - 1) / size)`; `Divisor::divide` is proved
   exact for every input (`classes::verify`, `classes::verify_slow`), so these are exactly the
   slots that overlap the pages. The unit test `purge_passes_tile_the_carved_pages` checks it
   against direct arithmetic for every class in three geometries, huge-page parts included.
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

**Review.** 2026-10-17, fresh adversarial reviewer: the protocol was accepted; three gaps were
found in the argument: exclusivity under a double free, the huge-page part not covered by the
cited proof, and no explored test of a carve or a second purger racing a purge. All three are
addressed above and await the same reviewer's check.
