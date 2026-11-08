# Safety ledger: `crates/cralloc/src/back/class.rs`

## BACK-CLASS-01: `region::purge_held(...)` in `purge_before_release`

**Preconditions** (`region::purge_held`). The caller holds the slot: its free bit is clear
because an allocation claimed it for the caller, who has not released it; nothing reads or
writes the slot's memory during the call or needs its contents after; and the slot is below
the watermark.

**Where it is called.** `free_slot`, the free of one large slot, calls `purge_before_release`
before it releases the slot's bit, and it purges when the slot is advised for huge pages because
its object grew there (its size word's flag, `Recorded::huge`, which only the slot's holder reads
or writes). The advice is taken back after the purge (`region::unadvise`), which changes no
contents; if the OS refuses that, the flag stays and the next allocation of the slot tries again
(`alloc_at`).

`free_slot` is private and has three callers:

- `Back::free_by_address`, crate-private, for a large class, with the region and slot that
  `slot_in` found for the pointer after reading the class from that region's tag. It is reached
  from `Heap::free_large` and the C `free`, for a pointer the program is giving up.
- `Back::free`, crate-private, likewise after `validate` has checked that the pointer lies in a
  region tagged for the class. Only tests take its large path; the heap frees small blocks by it.
- `move_large_with`, for a move that is not an outgrowth (`Move::Other`), after the copy, with the
  region and slot `validate` found for the block the move's caller gives up (BACK-CLASS-03). The
  slot's bit is still clear, and the move has returned only pages behind its copy, so nothing
  reads or needs the slot's memory any more.

A slot released resident is charged its pages whose clean bit is clear (`region::charge`); the
free itself writes no clean bit. Before it releases the slot it writes the slot's size word, as
its holder, with the fine count in `Recorded::freed` (`Back::stamp_slot`), which changes no
contents and which the hold's walk reads only as a hint (BACK-REGION-01, "A second caller").

A clean bit says the page has not been written since it was carved or last purged. Only three
things set one, each by the slot's holder (only the holder of every slot a bit describes writes
it): a purge; a range the holder returned whole (a shrink's tail, a piece behind a move's copy
before the move frees the slot, or the slack a claim trims, BACK-CLASS-08; `region::mark_clean`);
and the carve of a slot (`region::mark_fresh`, for the pages outside its block, past its extent or
below its stagger, which nothing has written). The claim (`region::settle_slot`) clears the bits
under what the new tenant may write, from its first page, and so does any change that widens it
(`region::cover`, from the first page the tenant may write, by its real start, and from the slot's
start where the slot is advised for huge pages, since a huge fault makes the pages below a block
resident).

A clean bit must be true, since more than the retained count and the purge walk's choices rest on
it: the purge that leaves every page of a free slot clean sets the slot's zero bit
(BACK-REGION-01, step 3). No zero bit is set by anything else here, and a claim takes a slot for
zero only from its zero bit, which only a purge sets. So a claim never reports pages as zero that
are not (`a_small_tenant_after_a_large_one_is_charged_what_is_resident`, and the random runs of
`large_charge`, which read every page after every step). Those runs use a shape that staggers by
lines alone; what a stagger by pages does to the bits has directed tests in `back::tests::kernels`
(`the_pages_a_stagger_passes_over_stay_clean`,
`a_block_at_its_slots_start_grows_back_over_clean_pages`).

**Proof.**

1. *The slot is below the watermark.* `slot_in` aborts unless the pointer is the start of a
   slot (or of its stagger) of the region's class and `slot < carved`, and the region is
   tagged for that class (checked by `validate`, or read from the tag by `free_by_address`).
   The watermark only grows.
2. *The caller holds the slot.* The program was given the slot by `Back::alloc`, which
   cleared its bit, or carved it with its bit never set, and is freeing it now, so by the
   allocator's contract (a block is freed once, by its owner, and not used afterwards) the bit
   is still clear and no one else has the slot. A clear bit is held by whoever cleared it until
   that holder sets it: the only writers of a leaf bit are claims (compare-and-swap from set to
   clear) and releases (set).
   No purge walk can touch the slot's pages meanwhile: `purge_unit` must first claim every
   slot touching its unit, and its claim of this slot fails while the bit is clear.
3. *No one uses the memory, and nothing in it is needed.* The program has given the block
   up. The allocator keeps no state in object memory. The pages returned are those wholly
   inside the slot (`Layout::slot_unit`), and a large slot is whole pages, so they are the slot
   and nothing else. On pages of 4 KiB every large class is whole pages (Kani,
   `a_large_slot_is_whole_pages`, which covers the design's geometry and not a kernel's with
   larger pages). On larger pages some classes are not (56 KiB on 16 KiB), and the heap never
   uses them: `heap::large::place` rounds a request up to a page before it finds the class, and
   the class of a multiple of a power of two is one too
   (`a_large_block_gets_a_slot_of_whole_pages_on_every_kernel`).
4. *The zero and clean bits.* `purge_held` sets the slot's zero bit and its pages' clean
   bits after the pages are returned and before `release_one` sets the free bit, which is the
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
- *The stamp of a late second free.* A free that leaves the slot resident writes its size word
  (`stamp_slot`, a load and a store) before `release_one` checks the bit; below a huge page,
  where no check of the bit comes before that, nothing has looked at the bit yet. If the slot
  was freed and claimed again in between, the late free's store can land after the new
  owner's claim wrote the word, and puts back the size it loaded: the old tenant's, or the new
  one's with a stamp beside it. The new owner's recorded size may then be wrong, which is what
  `realloc` copies and `malloc_usable_size` reports. That is the second case above, a double
  free of a slot handed out again, which then releases the new owner's slot as well; the
  store adds a wrong size to that corruption. Every purge stays inside the slot whatever the
  word says (BACK-CLASS-02, -05, -08); a `realloc` that copies by a wrong size is part of the
  corruption, and can read a stagger past the slot.

The free purges before it releases: a free slot's memory is returned only by a thread that holds
it, through a claim of its own or the program's. Released first and claimed back to purge, a
second free in that window would set the bit silently, and a third party could then allocate the
slot while it was being purged.

**The second call: a block left unwritten (`return_unwritten`).** `purge_before_release`
first calls `return_unwritten`, which calls `purge_held` for the same slot when the block's
size word says it was given zeroed from a slot known to be zero (`Recorded::unwritten`, set by
`mark_unwritten` and cleared by every claim, `alloc_at`, so read here it is this tenant's) and
a sample of its pages finds at most one in sixteen resident (BACK-CLASS-06). It runs in
`free_slot` before `release_one`, with the same region and slot, and reads the leaf bit and
aborts if it is set before it purges, as the advised path does. So steps 1 to 4 and the
double-free cases above hold for it word for word: the preconditions are about who holds the
slot, and the mark and the sample only decide whether to purge. Either outcome is a valid
free. A slot advised for huge pages is not marked (`mark_unwritten` passes a slot whose `huge`
flag is set), but a marked block that then grows densely in place has its slot advised
(`region::advise_grown` sets `huge`) and keeps the mark. `return_unwritten` declines any slot
whose `huge` flag is set, so such a free takes the advised path above. A class whose slots are
smaller than the floor is declined first, before the size word is read: no marked block lives
there. The mark may stay in a free slot's size word until the next claim clears it. Besides the
holder only the hold's walk reads the word, for its stamp, so a stale mark is acted on only by a
free of that slot, which is a double free and caught by the bit's check before any purge. When
`purge_held` returns `false` (the slot lies in a huge-page part, has no whole page, or the OS
refused) no bit was changed and the free goes on as an ordinary one. When it returns `true` the
slot is clean and zero, and `free_slot` releases it with no charge and no hold stamp, as for an
advised slot purged here.

**Tests.** `heap::tests::unwritten`: a block left unwritten goes back known zero and
uncounted, and the next tenant is not marked; a block written in full, and an unzeroed block,
are ordinary frees; the mark survives a resize in place; a marked block that moves returns its
old slot. `a_block_that_grows_gets_huge_pages_until_it_is_freed` (the advised slot goes
back whole at the free, reads as zero and says so, and its advice is taken back);
`an_advised_slot_is_purged_only_in_whole_huge_pages`; the model-based and stress tests of the
back end run the same path with contents checked.

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
same promise, from `Bound::resize` and C's `realloc_other`), so the owner
holds the slot or run for the whole call and is not using the block meanwhile: a resize has
the block exclusively, as `realloc` and `Allocator::shrink` require. Counted from the start
of the slot or run, which the block starts `into` bytes past (its stagger, or zero; a run is
never staggered), the range is `from..from + len` with `from = into + size` rounded up to a
page and the end `into + old` rounded up to a page, capped at the slot or run: whole pages,
inside the slot, and wholly past the new usable size (`Back::usable_size` reports the bytes
to the end of the page the object ends in, and the new size is recorded before the purge).
`into + size` is checked to fit the slot, so `from` is at most the slot's size. The owner asked
for the smaller size, so the bytes past it are given up. `validate` aborts unless the pointer is a
handed-out slot of the class; a run is found by its head tag, and the size is checked against its
length.

Nothing else can touch the range: for a slot, a purge walk must claim the slot first and
cannot while its bit is clear; a run's regions are tagged, so no class can be assigned them
and no other run can claim them.

No leaf or zero bit is written. When the purge of a slot's tail reports that the whole range
went back (`purge_part` answers `true` only then), the holder marks those pages clean
(`region::mark_clean`), so the free that follows charges only the pages kept; a later grow in
place clears them again (`region::cover`). The holder of the slot is the only writer of the
clean bits of pages inside it while it holds it. A clean bit never stands over a page that
holds data: the pages were just returned, and the tenant may not write past its new size
until a grow that clears them first. The zero bit describes a free slot, and is not touched.

**What a later grow sees.** The pages read as zero. A grow in place exposes them as bytes
past the old size, which `realloc` leaves indeterminate and the zeroing variants clear
themselves; nothing records them as holding the old contents.

**Tests.** `a_large_slot_that_shrinks_in_place_returns_its_tail`,
`a_run_that_shrinks_in_place_returns_its_tail`, `a_grown_blocks_free_is_a_free`,
`a_shrinks_move_frees_the_old_slot_as_its_free_would`, and through C
`realloc_shrinking_a_large_block_returns_its_tail`.

## BACK-CLASS-03: the copies and purges in `move_large_with`

**Blocks.** In `move_large_with`: the calls of `copy_window`, `region::purge_part` or `Space::purge`
on parts of `from`, and `grown_into` (BACK-CLASS-04). In `copy_window`:
`from.add(x).copy_to_nonoverlapping(to.add(x), ..)` for the runs it copies, `Space::purge` on whole
pages of `to` under a window's zero pages, and `is_zero` on the window's pages of `from`
(BACK-CLASS-04).

**Preconditions.** As for `copy_nonoverlapping`, `purge_part`, `Space::purge` and `is_zero`
(BACK-CLASS-04).

**Offsets.** A large block starts at its slot's start or at its stagger, under an eighth of
the slot and 4 KiB in (`policy::stagger_room`); `offset_in_slot` gives which. Every range below
that `purge_part` or `give_back` takes is counted from the slot's start (from the run's for a run,
which is never staggered).

**Proof.** `move_large` is an `unsafe fn`: its caller promises that `from` is a large block it
owns and gives up, readable for `len` bytes, that `to` is a block it owns, writable for `len`
bytes, whose contents it does not need, and disjoint from `from`, that nothing else uses
either range, and that if it says `zeroed` the `len` bytes at `to` read as zero. The two
callers are the moving paths of `Bound::resize` (HEAP-04, which passes the allocation's own
report) and C's `realloc_other` (MOD-01, which passes `false`), and both hold both blocks.

A slot its block outgrew (`Move::Outgrown`) and that the count of kept outgrown slots has
room for (`keep_outgrown`, true only for a slot, which counts the slot's charge before the
copy), and that is neither marked unwritten nor advised for huge pages, is kept: the move
returns nothing of `from` (steps 4 and 5 do not run), marks the size word
(`Recorded::outgrown`, written by the holder) and releases the slot resident and counted
already. The copies of steps 0 to 3 are the same, and keeping both copies resident is a
matter of memory, not of safety. Any other move (`Move::Other`) returns the pieces behind its
copy (step 4, without the last range) and frees the slot through `free_slot` (BACK-CLASS-01)
while the move's caller's claim on it still holds. That free returns the whole slot in two
cases: it is advised for huge pages, or its block was marked unwritten and the sample of its
extent (BACK-CLASS-06) finds at most one page in sixteen resident. On this path the sample
reads pages the move has already returned, which read as absent, so a block whose pieces went
back is likelier to be returned whole. Either outcome is a valid free: the slot is held, and
returning a held slot or releasing it resident are both frees of it whatever `mincore` says.
And the zero bit it may set is true: `purge_held` returns every whole page of the slot, those
already returned among them, before `mark_purged` sets the bit, so every page then reads as
zero (step 5).

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
   page inside `to`'s `len` bytes is a page of `to`'s slot or run, also where `to` is a small
   block, as when a block shrinks across the small classes. If the purge succeeds, the
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
4. *The purges of `from`.* Pieces end at multiples of the piece in the address space. Each range
   is either a part already copied that ends at such a multiple (from the slot's start for the
   first, which takes the pages a stagger passed over, whose contents no one needs, and an earlier
   tenant may have left; and from the previous multiple after that, which is page-aligned; to
   `into + boundary`) or, once and for an outgrown slot not kept only, everything from where the
   last of those ended (or from the slot's start, if there was none) to the end of the slot. Every
   byte of the block before that start has been copied. A block of one piece or less that crosses
   no multiple gives back only that last range, the whole slot, after its one copy. In a slot
   advised for huge pages `purge_part` may trim a range inward to whole huge pages
   (BACK-REGION-03), which only returns less, and then reports `false`, so step 5 does not count
   the range as gone. All lie inside `from`'s slot or run, whose start is page-aligned, and the
   caller holds it and has given it up, as in BACK-CLASS-01. The slot's bit is checked to be clear
   before anything is purged, so a block that is already free aborts instead. A range of a slot
   that went back whole is marked clean (`region::mark_clean`), by the holder, over pages that are
   now not resident: a clean bit stays true.
5. *The release.* The slot is marked zero only if every `give_back` reported its whole
   range returned: `purge_part` answers `true` only when the OS accepted the range it was
   given, untrimmed (`a_trimmed_purge_of_an_advised_slot_says_it_kept_part`), and the
   ranges together are every page of the slot, below the block too. So a slot marked zero
   has no stale byte in it, and the next zeroed allocation of it may skip clearing. (No move
   range is in fact trimmed, since a block under advice moves a huge page at a time at least,
   so its pieces end on huge-page boundaries; the argument does not depend on that.) If every
   purge succeeded, every page of the slot has gone back, and
   `mark_purged` and an uncounted release leave it exactly as `purge_held` does; if any did
   not, the slot is released as resident and counted by its pages that are not clean
   (`region::charge`), with no zero bit set and only the ranges that went back whole marked
   clean. A run is freed by `free_run`, which purges
   all of it (again, for the pieces already returned; harmless) to restore its zero
   promise. The release of a move that is not an outgrowth is `free_slot`'s
   (BACK-CLASS-01): if it returns the slot, `purge_held` has returned every whole page of
   it, the pieces behind the copy again included, so `mark_purged`'s zero bit is true for
   the same reason as `purge_held`'s at any free; if it does not, the slot is released
   resident, charged by its pages that are not clean, the pieces that went back whole having
   been marked clean by the move, with no zero bit set.

The double-free cases are those of BACK-CLASS-01, with the copy added to the window.

**Tests.** `a_moving_large_block_releases_its_old_pages_behind_the_copy`,
`a_moving_run_releases_its_old_pages_behind_the_copy`,
`a_sparse_block_stays_sparse_when_it_moves` (contents, residency, and a dirty destination),
`a_dense_block_with_a_hole_moves_whole_and_keeps_the_hole` (a hole across a piece boundary,
into a clean, a dirty and a staggered dirty destination),
`a_hole_is_copied_when_the_destinations_purge_fails`,
`a_staggered_block_moves_and_returns_its_whole_slot`,
`a_large_slot_is_charged_its_pages_that_are_not_clean`, the slots kept and returned in
`back::tests::outgrown` (among them
`a_block_of_several_pieces_is_kept_whole_within_the_bound_and_returned_piece_by_piece_past_it`,
`another_move_of_several_pieces_charges_only_what_stays_resident` and
`a_shrinks_move_frees_the_old_slot_as_its_free_would`),
`a_zeroed_grow_into_a_dirty_slot_reads_as_zero_past_what_it_kept`,
`an_advised_slot_is_purged_only_in_whole_huge_pages` (a block that does not start on a
piece boundary); the heap's and the C layer's `realloc` tests move large blocks through it
with contents checked.

## BACK-CLASS-04: `is_zero`, `mostly_data` and `grown_into`

**Blocks.** In `is_zero`, `core::slice::from_raw_parts(ptr, len)` and
`bytes.align_to::<u64>()`; in `mostly_data`, `is_zero(ptr.add(at), n)`; in `grown_into`'s closure,
`mostly_data` on the last piece of the `len` bytes; in `move_large_with`, the call of `grown_into`;
in `copy_window`, `is_zero(from.add(p), q - p)`.

**Preconditions.** `ptr..ptr + len` is readable and not written during the call (the functions'
`# Safety`); for `align_to`, that every bit pattern is valid for `u64`; and that the bytes are
initialised, since these are typed reads and the test branches on what they hold (below).

**Proof.** There are three callers. `grown_into`, from `move_large_with` before anything of `from`
is returned and with nothing else using `from` (BACK-CLASS-03), on the last piece of the `len`
bytes the caller promised readable: the offset `last` is at most `len`, and the closure runs only
inside `region::advise_grown`, so within `grown_into`'s call. `copy_window`, on each page `p..q` of
its window, at most `page` bytes long and inside those bytes, before the window is copied and so
before that page is returned (BACK-CLASS-03, step 1). And `resize_in_place`, on the last piece of a
growing block's first `old` bytes (BACK-CLASS-02). `mostly_data` calls `is_zero` on consecutive
pages with `at + n <= len`. So the slice is valid for its lifetime, the call. Any bit pattern is a
valid `u8` and a valid `u64`, and `align_to` places in its middle part only whole, aligned `u64`s
within the slice. Reading a page that was never written maps the kernel's shared zero page; it
changes no contents.

**Initialised bytes are assumed, not proved.** A program can put bytes in a block that Rust's model
calls uninitialised, such as a struct's padding or a `MaybeUninit`, and in that model comparing one
with zero is undefined. A copy carries such bytes as they are (MOD-01), but these reads look at
them. What stands in is the machine: every byte of the reservation holds a value, and the compiler
cannot carry what it knows of the program's store to these loads, since each is reached through a
function that is not inlined (`Bound::resize`, `realloc_other`). With a value to read the test is
right: a page is left out of a copy, or a block judged dense, only by what the page holds. We accept
this, and no test holds such bytes.

## BACK-CLASS-05: the copies and the purge in `unspill`

**Blocks.** In `resize_in_place`, the call of `unspill`. In `unspill`,
`self.space.resident(rest, huge - part)`, `spill.copy_to_nonoverlapping(aside, part)`,
`region::purge_part(host, region, slot, at, huge)`, and
`aside.copy_to_nonoverlapping(spill, part)`.

**Preconditions.** For `unspill`: its `# Safety`, that the caller owns the block, may now
use `new` bytes rounded up to the end of their last page, and does not read or write it during
the call, and that its contents are what it could use before, `old` bytes rounded up the same
way. For the copies: both ranges valid for `part` bytes, the source readable, the destination
writable, and the two not overlapping. For `purge_part`: the caller holds the slot, nothing
reads or writes the range during the call, and nothing in it is needed after. For
`Space::resident`: nothing writes the range during the call.

**Proof.** `resize_in_place` is an `unsafe fn` whose callers own the block and do not use it
during the call (BACK-CLASS-02). It calls `unspill` only when the block grows in place, after
`validate` found the slot and checked `into + new` within the class size, and after recording
the new size. What a block may use is its usable size (`Back::usable_size`): its recorded size
rounded up to the end of the page it ends in, which C callers are told and may write in full.
So the caller's contents end at `end`, `into + old` rounded up to a page, counted from the
slot's start, and the block may now use up to `into + new` rounded up to a page, which is at
most the class size, a whole number of pages, so it lies in the slot.

`at` is the start of the huge page holding `end`; `fresh` is `into + new` rounded up to a
page, the new usable end; `room` is the start of the huge page holding `fresh - 1`. Neither
falls below the slot's start, so neither subtraction wraps. `unspill` runs only if
`region::advise_grown` advised a whole huge page, so `into + new` reaches at least a huge page past
the slot's first huge-page boundary, `gap` bytes in, with `gap` under a huge page. And the block at
most doubled (`policy::grown_huge`), so `end >= into + old >= (into + new) / 2 >= (gap + huge) / 2`,
which is past `gap`. Slots are whole pages, and a region is aligned to at least a huge page wherever
huge pages are used, so these offsets mean the same in the address space, and every range below is
page-aligned.

`unspill` returns early unless `into < at`, `0 < part` with `part = end - at`, `part` at most a
page or the block's stagger rounded up to a page (a whole number of pages, since `end` and `at`
are), `at + huge <= room`, and `room + part <= fresh`; the two tests against `advised` decide only
whether the purge helps. So `spill` (offset `at`, `part` bytes) and `aside` (offset `room`, `part`
bytes) both lie inside what the block may now use, and `aside` starts at or past `at + huge`, past
the end of `spill`, so they do not overlap. `spill` holds the block's contents from `at` to `end`;
`aside` lies past `end`, in new room whose contents the caller does not need, so writing it
destroys nothing (a zeroed grow clears it after the resize, `Bound::grow_block`, HEAP-08). `rest`
is `at + part .. at + huge`, whole pages inside the block, which the caller does not write during
the call and nothing else can reach (below); `resident` only reads, and its answer only decides
whether the copies and the purge follow. The purge range is `at..at + huge`: whole pages, before
`room` and so inside what the block may use and inside the slot, held by the caller. Its only
contents are the `part` bytes now copied aside; the rest is past `end`. A purge may act on part of
the range even when it fails, so the bytes are copied back whatever it returns; after it, the
range reads as zero or as before, and the copy back writes the same `part` bytes at `spill`,
restoring the block's contents exactly. Nothing else can reach the range: the slot's bit stays
clear for the whole call, so no purge walk or claim can take it.

No leaf, zero or clean bit is written. The purged pages lie inside what the block may write,
whose clean bits `region::cover` clears right after; none is set here, so no clean bit stands
over a page with data, and the purged pages were never in a retained count, being the held
block's.

## BACK-CLASS-06: the sample in `return_unwritten`

**Block.** `self.space.resident(ptr, page)` for each sampled page.

**Preconditions** (`Space::resident`, `Os::resident`). The range is whole pages inside the
reservation (`Space::check` aborts otherwise), and nothing writes it during the call.

**Proof.** `return_unwritten` runs from `purge_before_release`, inside `free_slot`, while the
caller holds the slot: its leaf bit is clear for the program, which is freeing it (BACK-CLASS-01,
step 2). Each sampled page is `first + (2i + 1) * pages / (2 * samples)` for `i` below `samples`,
where `first` is the page holding the slot's stagger, no lower than `unit.start`
(`Layout::first_page`), `pages` is `end - first` or zero, and `end`, from `region::extent_of`, is
at most `unit.end`. So the page lies in `first..end`, inside the slot's whole pages
(`Layout::slot_unit`), and so inside the reservation, page-aligned and one page long; with no
pages there is no sample and no call. The program has given the block up and the allocator keeps
no state in it, so nothing writes it during the call. `mincore` reads only page tables and writes
one byte, into a local of the OS layer. Its answer decides only whether `purge_held` is called,
and both outcomes are valid frees whatever it says: a page swapped out or reclaimed reads as
absent and leads, at worst, to returning a slot whose contents nobody needs; a page reported
resident leads to an ordinary free. An error from the call (`Err`) is taken as "not left
unwritten", an ordinary free.

## BACK-CLASS-07: `self.move_large_with(from, to, len, zeroed, why, || {})` in `move_large`

**Proof.** `move_large_with`'s `# Safety` is "As for `Back::move_large`", and `move_large`
passes its own five arguments unchanged to the same back end. The one argument it adds is
`observe`, a closure that does nothing, which `move_large_with` calls between pieces; it reads
and writes no memory, so it cannot touch either range while the move runs.

## BACK-CLASS-08: `region::purge_part(...)` in `trim_slack`

**Block.** `region::purge_part(host, region, slot, from, len)` in `Back::trim_slack`, the trim
at the claim of a free large slot: the top `policy::PURGE_STEP_PAGES` at most of the pages past
both the new block's extent and the extent of the slot's last tenant that are not clean.

**Preconditions** (`region::purge_part`). The caller holds the slot (its free bit is clear on
the caller's behalf), nothing reads or writes the range during the call, and nothing in it is
needed after.

**Where it is called.** Only from `Back::alloc_at`, for a large class, after `tree::claim`
cleared the slot's free bit for this thread and `region::settle_slot` settled it, and before
`alloc_at` returns the block, so before anyone but this thread knows the slot is taken. The
`slack` it is given is the `Settled::slack` that `settle_slot` found for this slot in the same
call, unchanged; `trim_slack` raises its start to at most a step below its end, which only
shrinks it. Between the two, `alloc_at` writes the slot's size word and may take back its
huge-page advice and clear clean bits under the block (`region::unadvise`, `region::cover`),
none of which touches the slot's memory.

**Proof.**

1. *The caller holds the slot.* `tree::claim` cleared the slot's bit with a compare-and-swap
   against a value with it set, so this thread holds it until it sets it again (the argument
   of BACK-REGION-01, step 1); nothing in `alloc_at` sets it. No purge can claim it meanwhile,
   and no allocation can be handed it. The program does not know the block yet.
2. *The range lies inside the slot, past everything the new block may write.* `settle_slot`
   finds `slack` among the pages of `Layout::slot_unit(slot)`, the pages wholly inside the
   slot (a large slot is whole pages: BACK-CLASS-01, step 3), at or past
   `keep_from(layout, slot, unit, size, last)`, which is at least
   `extent_of(layout, slot, unit, size, huge)`: the page past the slot's start plus its stagger
   plus `size`, rounded up to a page (to a huge page if the slot is still advised, which only
   rounds up), clamped to the unit. The block `alloc_at` hands out starts at the slot's start
   or at its stagger, and the caller may write its usable size, `size` from that start rounded
   up to the end of its page (`usable`), so its last writable byte lies below that extent's
   page. The same holds for every later resize in place, which records the new size and clears
   the clean bits under it (`region::cover`) before the block may use it; a grow that reaches
   trimmed pages finds them zero, which `realloc` leaves indeterminate. `from` is
   `slack.start` in bytes less the slot's offset, at least zero since `slack.start` is at least
   the unit's first page, and `from + len` is `slack.end` in bytes less that offset, at most the
   slot's size; `purge_part` checks that too, and works on a subrange when the slot is advised
   (BACK-REGION-03).
3. *Nothing reads or writes the range, and nothing in it is needed.* The slot was free: its
   last tenants gave up its pages at their frees, and the allocator keeps no state in object
   memory. The new tenant's block does not reach the range (step 2), and it has not been handed
   out. A claim that reports the slot zero has every page clean (the zero bit is set only with
   every page clean, BACK-REGION-01, step 3), so `settle_slot` finds no slack and nothing is
   trimmed; and the trimmed pages read as zero anyway.
4. *The bits.* When `purge_part` says the whole range went back, the holder marks those pages
   clean (`region::mark_clean`): it holds the one slot they lie in, which is the rule clean
   bits have everywhere, and a clean bit over them is true until a widening of the block
   clears it (step 2). No zero bit is written: the slot is held. When the call is refused,
   wholly or in part, nothing is marked, and the pages stay not clean, so the block's free
   charges them.

**What a double free can do here.** As for any claim: if the program frees the slot's last
tenant a second time after this claim, the release finds the bit clear and sets it, and another
thread may claim the slot while this one trims it. The other claimer's block may then reach
into the range this thread returns, and its contents there read as zero. That is the program's
double free corrupting a block, as it would through the free alone (BACK-CLASS-01, the second
case); the trim adds the zeroing, of pages the first claimer would not write.

**A slot that kept its huge-page advice** (only a refused `unadvise` leaves one). Its claim
computes both extents in whole huge pages, and `purge_part` cuts the step inward to whole huge
pages inside the slot's interior. A step is less than a huge page on every real kernel, so nothing
of it goes there. Part of a step can still lie outside the slot's whole huge pages, in its
unaligned tail, which the advice never covered: that part is returned and `purge_part` reports
false, so no clean bit is set and the free charges those pages though they are gone. An
over-count, never a page in use.

**An accepted limit: a refused purge at the free, then a successful `unadvise`.** The free of
an advised slot purges it whole and then takes the advice back. If the purge is refused and
the `unadvise` succeeds, the slot is free, not advised and resident, possibly in huge pages
the kernel has not split. A later claim's step may then return part of such a huge page,
which frees nothing until the kernel splits it and stays charged to the cgroup meanwhile, as
a shrink in place over a huge page does (`region`, "Huge pages for grown blocks"). It needs a
refused `madvise` first; we accept it rather than track it.

**Accounting, not safety.** The claim takes the slot's whole charge off the retained count: the
pages under the block are live, the top step of the slack goes back, and the rest of the slot's
resident pages stay resident and in no count until the block's free charges them again, or the
slot's next claim trims another step.

**Tests.** `back::tests::slack`: a claim returns what the tenants before the last left and
keeps the last tenant's room, and the count follows; a trim returns a step at most, from the
top, and the next claim the next step; a hot slot's resident part no longer climbs over two
hundred tenants of random sizes; a slot advised for huge pages, or left
unwritten, has nothing to trim and the claim makes no call; a kept outgrown slot keeps the
room its block used and its count falls as before; a refused trim, whole or half done,
leaves its pages not clean, charged by the free and trimmed by the next claim.
`large_path::the_claim_counts_and_clears_what_two_passes_did` checks the slack the pass
finds against direct arithmetic over random clean bits and last tenants. The random runs of
`large_charge` (clean bits true, the count equal to the clean bits, every zero slot zero,
with purges and advice failing) and the exploration `an_age_walk_races_large_tenants`, whose
tenants trim (`TestHost::purge` fails a schedule in which a purger does not hold every slot it
returns).

Reviewed: 5a8ade1
