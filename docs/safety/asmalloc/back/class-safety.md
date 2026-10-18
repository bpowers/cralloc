# Safety ledger: `crates/asmalloc/src/back/class.rs`

## BACK-CLASS-01: `region::purge_held(...)` in `purge_before_release`

**Preconditions** (`region::purge_held`). The caller holds the slot: its free bit is clear
because an allocation claimed it for the caller, who has not released it; nothing reads or
writes the slot's memory during the call or needs its contents after; and the slot is below
the watermark.

**Where it is called.** `Back::free` of a large class calls `purge_before_release` before it
releases the slot's bit, when keeping the slot resident would take the large classes'
retained count over its cap, or when the slot is advised for huge pages because its object
grew there (its size word's flag, `Recorded::huge`, which only the slot's holder reads or
writes). Which of the two reasons applies changes nothing below: the same thread holds the
same slot. The advice is taken back after the purge (`region::unadvise`), which changes no
contents. `Back::free` is crate-private (`pub(crate)`), so these are its only callers, and
it is reached from `Heap::free_large`, the C `free` and the depot's `free_one` (a thread with
no CPU giving up a small block), each for a pointer the program is giving up.

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
`self.space.purge(start, len)` for a run, when a large object shrinks in place; and, when a
slot's object grows in place, `mostly_data` on the last piece of its first `old` bytes, which
only reads them (BACK-CLASS-04): the owner holds the block and does not use it during the
resize, and `old` is its recorded size, within the slot.

**Preconditions.** For `purge_part`: the caller holds the slot, nothing reads or writes the
range during the call, and nothing in it is needed after. For `Space::purge`: no thread reads
or writes the range during the call and nothing in it is needed.

**Proof.** `resize_in_place` is an `unsafe fn` whose contract is that `ptr` is a live large
block the caller owns, unused during the call, and that the caller gives up every byte past
`size`. Its callers are the heap's for a block its owner is resizing
(`Heap::resize_large_in_place`, `Heap::resize_in_place_by_address`, both `unsafe fn`s with the
same promise, from `Heap::resize` and C's `realloc`), so the owner
holds the slot or run for the whole call and is not using the block meanwhile: a resize has
the block exclusively, as `realloc` and `Allocator::shrink` require. Counted from the start
of the slot or run, which the block starts `into` bytes past (its stagger, or zero; a run is
never staggered), the range is `from..from + len` with `from = into + size` rounded up to a
page and the end `into + old` rounded up to a page, capped at the slot or run: whole pages,
inside the slot, and wholly past the new usable size (`Back::usable_size` reports the bytes
to the end of the page the object ends in, and the new size is recorded before the purge).
`into + size` is checked to fit the slot, so `from` is at most the slot's size. The owner asked for the smaller size, so the
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

**Blocks.** `from.copy_to_nonoverlapping(to, ..)`, whole, or a page at a time within each
piece, skipping pages of `from` that `is_zero` finds all zero; `Space::purge(to, ..)` once
before a piecewise copy into a destination not known to be zero; `region::purge_part` or
`Space::purge` on parts of `from`.

**Preconditions.** As for `copy_nonoverlapping`, `purge_part`, `Space::purge` and `is_zero`
(BACK-CLASS-04).

**Offsets.** A large block starts at its slot's start or at its stagger, less than a page
in; `offset_in_slot` gives which. Every range below that `purge_part` or `give_back` takes is
counted from the slot's start (from the run's for a run, which is never staggered).

**Proof.** `move_large` is an `unsafe fn`: its caller promises that `from` is a large block it
owns and gives up, readable for `len` bytes, that `to` is a block it owns, writable for `len`
bytes, whose contents it does not need, and disjoint from `from`, that nothing else uses
either range, and that if it says `zeroed` the `len` bytes at `to` read as zero. The two
callers are the moving paths of `Heap::resize` (HEAP-04, which passes the allocation's own
report) and C's `realloc` (MOD-01, which passes `false`), and both hold both blocks.

0. *The destination purge.* When `zeroed` is false and the block is more than one piece,
   the pages `to..to + len` touches are purged before anything is copied. `to` holds more
   than a piece, so it is a large slot or a run, whole pages; it starts at most a stagger,
   under a page, past its slot's first page, and `into + len` fits the slot, so the pages
   are the caller's slot's own; nothing else uses it and its contents are not
   needed, which is `Space::purge`'s precondition. Afterwards it reads as zero. If the purge
   fails nothing is known to be zero, but nothing relies on the failure path either: see the
   skip below.
1. *The copies.* Each copies at most a page at offset `at` with `at + n <= len`, inside both
   ranges, and the bytes read have not been returned: a part of `from` goes back only after
   the piece that covers it has been copied, and the range past `len` goes back only after
   the last copy. A page is skipped only when every byte of it in `from` is zero, and then
   `to` already reads as zero there, either because the caller said so or because of step 0.
   A failed destination purge would leave stale bytes in skipped pages: a wrong value in the
   caller's new block, not a memory error. (The destination purge never fails in practice;
   `MADV_DONTNEED` fails only on a bad range, which `Space::purge` rules out by aborting.)
2. *The purges.* Pieces end at multiples of the piece in the address space. Each range is
   either a part already copied that ends at such a multiple (`into + done` rounded down to a
   page, to `into + boundary`: `done` is zero, which rounds down to the slot's start and so
   takes the stagger's padding, which holds nothing, or the previous multiple, which is
   page-aligned) or, once, everything from the last multiple at or below `from + len` (or
   from the slot's start, if there is none) to the end of the slot or run. Every byte of the
   block before that start has been copied. In a slot advised for huge pages
   `purge_part` may trim a range inward to whole huge pages (BACK-REGION-03), which only
   returns less, and then reports `false`, so step 3 does not count the range as gone. All
   lie inside `from`'s slot or run, whose start is page-aligned, and the caller holds it and
   has given it up, as in BACK-CLASS-01. The slot's bit is checked to be clear before
   anything is purged, so a block that is already free aborts instead.
3. *The release.* The slot is marked zero only if every `give_back` reported its whole
   range returned: `purge_part` answers `true` only when the OS accepted the range it was
   given, untrimmed (`a_trimmed_purge_of_an_advised_slot_says_it_kept_part`), and the
   ranges together are every page of the slot past the copy's start. So a slot marked zero
   has no stale byte in it, and the next zeroed allocation of it may skip clearing. (Today
   no move range is ever trimmed, since pieces end on huge-page boundaries and a piece is a
   huge page; the argument does not depend on that.) If every purge succeeded, every page of
   the slot has gone back, and
   `mark_purged` and an uncounted release leave it exactly as `purge_held` does; if any did
   not, the slot is released as resident and counted, with no zero or clean bit set, which
   only over-counts the parts that did go back. A run is freed by `free_run`, which purges
   it again (harmless) to restore its zero promise.

The double-free cases are those of BACK-CLASS-01, with the copy added to the window.

**Tests.** `a_moving_large_block_releases_its_old_pages_behind_the_copy`,
`a_moving_run_releases_its_old_pages_behind_the_copy`,
`a_sparse_block_stays_sparse_when_it_moves` (contents, residency, and a dirty destination),
`a_zeroed_grow_into_a_dirty_slot_reads_as_zero_past_what_it_kept`,
`an_advised_slot_is_purged_only_in_whole_huge_pages` (a block that does not start on a
piece boundary); the heap's and the C layer's `realloc` tests move large blocks through it
with contents checked. Pieces on address boundaries, the trimming in `purge_part`, and the
zero-page skip with its destination purge: awaiting review.

## BACK-CLASS-04: `is_zero`, `mostly_data` and `grown_into`

**Blocks.** In `is_zero`, `core::slice::from_raw_parts(ptr, len)` and
`bytes.align_to::<u64>()`; in `mostly_data`, `is_zero(ptr.add(at), n)`; in `grown_into`'s
closure, `mostly_data(from, len, page)`; in `move_large_with`, the call of `grown_into`.

**Preconditions.** `ptr..ptr + len` is readable and not written during the call (the
functions' `# Safety`); for `align_to`, that every bit pattern is valid for `u64`.

**Proof.** The callers are in `move_large_with`, before anything of `from` is returned and
with nothing else using `from` (BACK-CLASS-03): `grown_into` on the `len` bytes the caller
promised readable, which hands them to `mostly_data` (only from the closure that
`region::advise_grown` calls before it returns, so within `grown_into`'s call), which calls
`is_zero` on consecutive pages with `at + n <= len`; the copy loop, which calls `is_zero`
on a page of `from` at most `page` bytes long inside those bytes before that page is
returned; and `resize_in_place`, on the last piece of a growing block's first `old` bytes
(BACK-CLASS-02). So the slice is valid for its lifetime, the call.
Any bit pattern is a valid `u8` and a valid `u64`, and `align_to` places in its middle part
only whole, aligned `u64`s within the slice. Reading a page that was never written maps the
kernel's shared zero page; it changes no contents. Awaiting review.
