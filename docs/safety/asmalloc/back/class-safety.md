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
  by the other and aborts. Detected, unless a third thread's allocation claims the slot
  between the two releases. Then the late free's `madvise` zeroes the new owner's data, its
  `mark_purged` sets the zero bit of a slot that is live, and its release frees that slot
  under its owner: corruption of a live block, from the program's double free. Without the
  purge the same interleaving frees the new owner's slot but leaves its bytes; the `madvise`
  adds the zeroing and the wrong zero bit, and makes the time between entering `free` and
  releasing the bit longer.

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

**Review.** 2026-10-17, fresh adversarial reviewer: accepted, with the stronger statement of
the third case above added at the reviewer's request.

The count that decides whether to purge is read here and raised by the release, so several
threads freeing at once can each keep their slot and together pass the cap by one slot per
thread but the first. That is policy, not safety.

## BACK-CLASS-02: the purges in `resize_in_place`

**Blocks.** `region::purge_part(&host, region, slot, from, len)` for a slot, and
`self.space.purge(start, len)` for a run, when a large object shrinks in place.

**Preconditions.** For `purge_part`: the caller holds the slot, nothing reads or writes the
range during the call, and nothing in it is needed after. For `Space::purge`: no thread reads
or writes the range during the call and nothing in it is needed.

**Proof.** `resize_in_place` is called by the heap for a live large block that its owner is
resizing (`Heap::resize_large_in_place`, `Heap::resize_in_place_by_address`), so the owner
holds the slot or run for the whole call and is not using the block meanwhile: a resize has
the block exclusively, as `realloc` and `Allocator::shrink` require. The range is
`from..from + len` with `from = size` rounded up to a page and the end the old recorded size
rounded up to a page, capped at the slot or run: whole pages, inside the block, and wholly
past the new usable size (`Back::usable_size` reports the recorded size rounded up to a page,
and the new size is recorded before the purge). The owner asked for the smaller size, so the
bytes past it are given up. `validate` aborts unless the pointer is a handed-out slot of the
class; a run is found by its head tag, and the size is checked against its length.

Nothing else can touch the range: for a slot, a purge walk must claim the slot first and
cannot while its bit is clear; a run's regions are tagged, so no class can be assigned them
and no other run can claim them.

No bitmap is written. The clean and zero bits of a live slot are stale until its next free
and are not consulted while it is live; after the free they may say "resident" of pages that
are not, which costs an over-count and a redundant `madvise`, never a wrong zero.

**What a later grow sees.** The pages read as zero. A grow in place exposes them as bytes
past the old size, which `realloc` leaves indeterminate and the zeroing variants clear
themselves; nothing records them as holding the old contents.

**Tests.** `a_large_slot_that_shrinks_in_place_returns_its_tail`,
`a_run_that_shrinks_in_place_returns_its_tail`, and through C
`realloc_shrinking_a_large_block_returns_its_tail`. Awaiting review.

## BACK-CLASS-03: the copies and purges in `move_large_with`

**Blocks.** `from.copy_to_nonoverlapping(to, ..)`, whole or a piece at a time;
`region::purge_part` or `Space::purge` on parts of `from`.

**Preconditions.** As for `copy_nonoverlapping`, `purge_part` and `Space::purge`.

**Proof.** `move_large` is an `unsafe fn`: its caller promises that `from` is a large block it
owns and gives up, readable for `len` bytes, that `to` is writable for `len` bytes and
disjoint from it, and that nothing else uses either range. The two callers are the moving
paths of `Heap::resize` (HEAP-04) and C's `realloc` (MOD-01), which hold both blocks.

1. *The copies.* Each copies `step` bytes at offset `done` with `done + step <= len`, inside
   both ranges. The bytes read have not been returned: a part of `from` goes back only after
   the piece that covers it has been copied, and the range past `len` goes back only after
   the last copy.
2. *The purges.* Each range is either a whole piece already copied (`done..done + step`,
   with `step` a piece, which is a page multiple, at an offset that is a multiple of the
   piece) or, once, everything from the last whole piece to the end of the slot or run. All
   lie inside `from`'s slot or run, whose start is page-aligned, and the caller holds it and
   has given it up, as in BACK-CLASS-01. The slot's bit is checked to be clear before
   anything is purged, so a block that is already free aborts instead.
3. *The release.* If every purge succeeded, every page of the slot has gone back, and
   `mark_purged` and an uncounted release leave it exactly as `purge_held` does; if any did
   not, the slot is released as resident and counted, with no zero or clean bit set, which
   only over-counts the parts that did go back. A run is freed by `free_run`, which purges
   it again (harmless) to restore its zero promise.

The double-free cases are those of BACK-CLASS-01, with the copy added to the window.

**Tests.** `a_moving_large_block_releases_its_old_pages_behind_the_copy`,
`a_moving_run_releases_its_old_pages_behind_the_copy`; the heap's and the C layer's
`realloc` tests move large blocks through it with contents checked. Awaiting review.
