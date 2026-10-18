# Safety ledger: `crates/asmalloc/src/back/class.rs`

## BACK-CLASS-01: `region::purge_held(...)` in `purge_before_release`

**Preconditions** (`region::purge_held`). The caller holds the slot: its free bit is clear
because an allocation claimed it for the caller, who has not released it; nothing reads or
writes the slot's memory during the call or needs its contents after; and the slot is below
the watermark.

**Where it is called.** `Back::free` of a large class calls `purge_before_release` before it
releases the slot's bit, when keeping the slot resident would take the large classes'
retained count over its cap. `Back::free` is reached from `Heap::free_large` and the C
`free`, for a pointer the program is giving up.

**Proof.**

1. *The slot is below the watermark.* `validate` aborts unless the pointer is the start of a
   slot of the class in a region tagged for it and `slot < carved`. The watermark only grows.
2. *The caller holds the slot.* The program was given the slot by `Back::alloc`, which
   cleared its bit, and is freeing it now, so by the allocator's contract (a block is freed
   once, by its owner, and not used afterwards) the bit is still clear and no one else has
   the slot. A clear bit is held by whoever cleared it until that holder sets it: the only
   writers of a leaf bit are claims (compare-and-swap from set to clear) and releases (set).
   No purge walk can touch the slot's pages meanwhile: `purge_unit` must first claim every
   slot touching its unit, and its claim of this slot fails while the bit is clear.
3. *No one uses the memory, and nothing in it is needed.* The program has given the block
   up. The allocator keeps no state in object memory. The pages returned are those wholly
   inside the slot (`Layout::slot_unit`); a large slot is whole pages in every geometry in
   use (Kani, `a_large_slot_is_whole_pages`), so they are the slot and nothing else.
4. *The zero and clean bits.* `purge_held` sets the slot's zero bit and its pages' clean
   bits after the pages are returned and before `release_all` sets the free bit, which is the
   rule those bitmaps have everywhere: written only while the writer holds every slot they
   describe. The next claim of the slot reads the zero bit after it has the free bit.

**What a double free can do here.** The contract in step 2 is the program's, so this is what
breaking it does, compared with a free that does not purge.

- *The slot is already free when this free arrives* (freed twice in a row). Detected:
  `purge_before_release` reads the bit before it purges and aborts if it is set, and
  `release_all` would abort on the same bit. Nothing is purged.
- *The slot was freed and allocated again before this free arrives.* The bit is clear, the
  slot belongs to its new owner, and this free purges and releases it. Undetectable here, as
  it is in every allocator without per-object tags, and as it is on the path that does not
  purge, which would release the new owner's slot just the same. The purge adds that the new
  owner's contents read as zero from then on, where without it they would survive until the
  slot was handed out again.
- *A second free of the same pointer arrives during this free's `madvise`.* The bit is clear
  (this free has not released it yet), so the second free takes the same path: it may purge
  the same pages again, which is harmless, and then one of the two releases finds the bit set
  by the other and aborts. Detected, unless an allocation claims the slot between the two
  releases; then it is the case above. That last window exists without the purge too; the
  `madvise` only makes the time between entering `free` and releasing the bit longer.

The earlier form of this path released the slot first and then claimed it back to purge it,
which held a claim across the `madvise` on a slot that was nominally free: a second free in
that window set the bit silently and a third party could then allocate the slot while it was
being purged. Purging before the release removes that: at no point is a free slot's memory
returned by a thread that does not hold it through a claim of its own or the program's.

**Tests.** `freed_large_slots_have_a_small_cap_of_their_own` (kept slots stay resident, purged
ones read as zero and say so, a slot bigger than the cap goes back at once);
`racing_large_frees_leave_the_large_count_over_its_cap_with_nothing_free` (64 threads
allocating and freeing slots that are always purged; the model-based and stress tests of the
back end run the same path with contents checked).

**Review.** Awaiting review. Written 2026-10-17 with the change.
