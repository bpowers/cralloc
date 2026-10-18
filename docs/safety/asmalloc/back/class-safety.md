# Safety ledger: `crates/asmalloc/src/back/class.rs`

## BACK-CLASS-01: `region::purge_held(...)` in `purge_before_release`

**Preconditions** (`region::purge_held`). The caller holds the slot: its free bit is clear
because an allocation claimed it for the caller, who has not released it; nothing reads or
writes the slot's memory during the call or needs its contents after; and the slot is below
the watermark.

**Where it is called.** `free_slot`, the free of one large slot, calls `purge_before_release`
before it releases the slot's bit, and it purges only when the slot is advised for huge pages
because its object grew there (its size word's flag, `Recorded::huge`, which only the slot's
holder reads or writes) and keeping it would take retained large-slot memory past the budget
for kept grown blocks (`keeps_grown`, which reads a count and nothing of the slot). An earlier
form also purged when keeping any slot would take a large-class retained count over its cap,
and a later one purged every advised slot; each change only narrows or widens when the block
runs, without changing what holds when it does. A kept advised slot is released with its
advice and flag; the next allocation of it takes them back (`alloc_at`), and only the holder
of a slot reads the flag, so a free slot that is advised changes no other path: the purge walk
purges free slots by their pages and never reads it, and `purge_part` acts on held slots. The advice is taken back after the purge
(`region::unadvise`), which changes no contents. `free_slot` is private and called by
`Back::free` and `Back::free_by_address` for a large class, both crate-private
(`pub(crate)`), with the region and slot that `slot_in` found for the pointer: `Back::free`
after `validate` has checked the pointer lies in a region tagged for the class, and
`free_by_address` after reading the class from that region's tag. They are reached from
`Heap::free_large` and the C `free`, each for a pointer the program is giving up.

**Proof.**

1. *The slot is below the watermark.* `slot_in` aborts unless the pointer is the start of a
   slot (or of its stagger) of the region's class and `slot < carved`, and the region is
   tagged for that class (checked by `validate`, or read from the tag by `free_by_address`).
   The watermark only grows.
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

**Tests.** `a_block_that_grows_gets_huge_pages_until_it_is_freed` (the advised slot goes
back whole at the free, reads as zero and says so, and its advice is taken back);
`an_advised_slot_is_purged_only_in_whole_huge_pages`; the model-based and stress tests of the
back end run the same path with contents checked.

**Review.** 2026-10-17, fresh adversarial reviewer: accepted, with the stronger statement of
the third case above added at the reviewer's request.

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
same promise, from `Bound::resize` and C's `realloc`), so the owner
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
`realloc_shrinking_a_large_block_returns_its_tail`.

## BACK-CLASS-03: the copies and purges in `move_large_with`

**Blocks.** In `move_large_with`: the calls of `copy_window`, and `region::purge_part` or `Space::purge` on
parts of `from`. In `copy_window`: `from.add(x).copy_to_nonoverlapping(to.add(x), ..)` for
the runs it copies, `Space::purge` on whole pages of `to` under a window's zero pages, and
`is_zero` on the window's pages of `from` (BACK-CLASS-04).

**Preconditions.** As for `copy_nonoverlapping`, `purge_part`, `Space::purge` and `is_zero`
(BACK-CLASS-04).

**Offsets.** A large block starts at its slot's start or at its stagger, less than a page
in; `offset_in_slot` gives which. Every range below that `purge_part` or `give_back` takes is
counted from the slot's start (from the run's for a run, which is never staggered).

**Proof.** `move_large` is an `unsafe fn`: its caller promises that `from` is a large block it
owns and gives up, readable for `len` bytes, that `to` is a block it owns, writable for `len`
bytes, whose contents it does not need, and disjoint from `from`, that nothing else uses
either range, and that if it says `zeroed` the `len` bytes at `to` read as zero. The two
callers are the moving paths of `Bound::resize` (HEAP-04, which passes the allocation's own
report) and C's `realloc` (MOD-01, which passes `false`), and both hold both blocks.

Within the budget for kept grown blocks (`keeps_grown`, true only for a slot) the move
returns nothing of `from` (steps 4 and 5 do not run) and releases the slot as a plain free
does, counted and resident; the copies of steps 0 to 3 are the same, and keeping both copies
resident is a matter of memory, not of safety.

0. *The windows.* Every block, whatever its length, is copied by `copy_window` over
   consecutive, disjoint ranges `start..end` of block offsets that together are `0..len`, in
   increasing order, each inside one piece; a window is at most `WINDOW` pages of `from`'s
   address space, which its bitmap of zero pages has room for. So when a window runs, the
   move has written nothing of `to` at or past `start`, and returned nothing of `from` at or
   past `start` (step 4). Offsets below are block offsets, counted from `from` and from `to`
   alike.
1. *The zero test.* Each of the window's pages, `p..q` (the window's ends cut the first and
   the last), lies inside `start..end`, so inside the `len` readable bytes of `from`, which
   nothing writes (BACK-CLASS-04).
2. *The destination purge.* Only in a window that has a zero page, and only when the caller
   did not say `zeroed` and no purge of `to` has failed (`Dest::Dirty`). The range is the
   whole pages of `to` inside `to + lo'..to + hi'`, where `lo'` is the start of the
   window's first zero page and `hi'` the end of its last, so it lies inside
   `to + start..to + end`: pages of the caller's block `to`, whose contents it does not need
   and which the move has not written (step 0); nothing else uses them. That is
   `Space::purge`'s precondition. The range is page-aligned by construction, and a whole
   page inside `to`'s `len` bytes is a page of `to`'s slot or run. If the purge succeeds, the
   range reads as zero. It can fail: `MADV_DONTNEED` returns `EINVAL` on locked memory, so in
   a process that called `mlockall` it fails every time; then nothing is left out in this
   window and the move makes no further purge of `to` (`Dest::Unpurgeable`).
3. *The copies.* `clear` is the range of offsets at which `to` reads as zero and the move has
   written nothing: `start..end` when the caller said `zeroed`, the purged range when step 2
   succeeded, and empty otherwise. The window is copied as runs `next..s` between the parts
   left out, then `next..end`. A part left out is a zero page's `p..q` cut to `clear`, so
   every byte left out is zero in `from` and reads as zero in `to`. Parts left out are
   disjoint and taken in increasing order, so the runs copied are disjoint, inside
   `start..end`, and with them cover the window. Each copy is inside both blocks' `len`
   bytes, which do not overlap, and reads bytes of `from` not yet returned. When `clear` is
   empty, which is always so after a failed purge, every byte is copied, so no stale byte of
   `to` survives (`a_block_moved_into_a_dirty_slot_arrives_whole_when_purges_fail`,
   `a_hole_is_copied_when_the_destinations_purge_fails`).
4. *The purges of `from`.* Pieces end at multiples of the piece in the address space. Each range is
   either a part already copied that ends at such a multiple (`into + done` rounded down to a
   page, to `into + boundary`: `done` is zero, which rounds down to the slot's start and so
   takes the stagger's padding, which holds nothing, or the previous multiple, which is
   page-aligned) or, once and for a slot only, everything from the last multiple at or below
   `from + len` (or from the slot's start, if there is none) to the end of the slot. Every
   byte of the block before that start has been copied. A block of one piece or less that
   crosses no multiple gives back only that last range, the whole slot, after its one
   copy. In a slot advised for huge pages
   `purge_part` may trim a range inward to whole huge pages (BACK-REGION-03), which only
   returns less, and then reports `false`, so step 5 does not count the range as gone. All
   lie inside `from`'s slot or run, whose start is page-aligned, and the caller holds it and
   has given it up, as in BACK-CLASS-01. The slot's bit is checked to be clear before
   anything is purged, so a block that is already free aborts instead.
5. *The release.* The slot is marked zero only if every `give_back` reported its whole
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
   all of it (again, for the pieces already returned; harmless) to restore its zero
   promise.

The double-free cases are those of BACK-CLASS-01, with the copy added to the window.

**Tests.** `a_moving_large_block_releases_its_old_pages_behind_the_copy`,
`a_moving_run_releases_its_old_pages_behind_the_copy`,
`a_sparse_block_stays_sparse_when_it_moves` (contents, residency, and a dirty destination),
`a_dense_block_with_a_hole_moves_whole_and_keeps_the_hole` (a hole across a piece boundary,
into a clean, a dirty and a staggered dirty destination),
`a_hole_is_copied_when_the_destinations_purge_fails`,
`a_staggered_block_moves_and_returns_its_whole_slot`,
`grown_blocks_are_kept_within_their_budget`,
`a_zeroed_grow_into_a_dirty_slot_reads_as_zero_past_what_it_kept`,
`an_advised_slot_is_purged_only_in_whole_huge_pages` (a block that does not start on a
piece boundary); the heap's and the C layer's `realloc` tests move large blocks through it
with contents checked.

## BACK-CLASS-04: `is_zero`, `mostly_data` and `grown_into`

**Blocks.** In `is_zero`, `core::slice::from_raw_parts(ptr, len)` and
`bytes.align_to::<u64>()`; in `mostly_data`, `is_zero(ptr.add(at), n)`; in `grown_into`'s
closure, `mostly_data(from, len, page)`; in `move_large_with`, the call of `grown_into`;
in `copy_window`, `is_zero(from.add(p), q - p)`.

**Preconditions.** `ptr..ptr + len` is readable and not written during the call (the
functions' `# Safety`); for `align_to`, that every bit pattern is valid for `u64`.

**Proof.** The callers are in `move_large_with`, before anything of `from` is returned and
with nothing else using `from` (BACK-CLASS-03): `grown_into` on the `len` bytes the caller
promised readable, which hands them to `mostly_data` (only from the closure that
`region::advise_grown` calls before it returns, so within `grown_into`'s call), which calls
`is_zero` on consecutive pages with `at + n <= len`; `copy_window`, which calls `is_zero`
on each page `p..q` of its window, at most `page` bytes long and inside those bytes, before
the window is copied and so before that page is returned (BACK-CLASS-03, step 1); and `resize_in_place`, on the last piece of a growing block's first `old` bytes
(BACK-CLASS-02). So the slice is valid for its lifetime, the call.
Any bit pattern is a valid `u8` and a valid `u64`, and `align_to` places in its middle part
only whole, aligned `u64`s within the slice. Reading a page that was never written maps the
kernel's shared zero page; it changes no contents.

Reviewed: 415b07e
