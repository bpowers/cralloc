# Safety ledger: `crates/cralloc/src/back/host.rs`

`ClassHost` is the protocols' view of one class of the real heap. It turns metadata word
numbers into references into the reservation, and forwards purges to `Space`.

**The `ClassHost` invariant.** `layout` is `Layout::new(space.plan().geometry(), class)` for the
host's class, and every `RegionId` a caller passes is a region this class has named: a carving
word of the class has held it. Such a region is the class's for good. `Back::host` is the only
constructor, and it builds the layout that way. Region ids reach a host only from:

- the class's carving word, which holds only a region this class's `assign` took from the table
  with a compare-and-swap from `Unassigned` to `Class(class)` and then named with its swing;
- the class's summary tree (`tree::child` of a class word), whose bits are set only by the walk
  up from a leaf word of one of the class's regions;
- `Tags::at` of where a pointer into the region falls, whose table tag `validate`, the batch
  checker or the free by address has just checked is this class: a pointer into a region
  exists only once a class has named it and carved from it, so its tag is the naming class's;
- the published tag (`Tags::published`), which every walk that finds regions by their number
  reads (`Back::purgeable`, `Back::hold_region` and the hold's plan, `Back::lower_walked`,
  `Back::raise_marked`, `Back::inspect`). Only the thread whose carving swing named the region
  writes it, after the swing, and only with that class (`Back::assign`). The one other writer
  is a fork child's repair (`Back::publish_named`), whose read of the table may be stale, since
  threads of the child may run beside it: it publishes `class` only if the class's carving word
  names the region (only a won swing of the class puts it there), or if the region has been
  carved from and the table, read again after that, still says `class`. A nonzero `carved` read
  through `class`'s host means some class has carved from the region, whichever host it was
  read through: two small classes share the region's header, which only carves write; a large
  class's header is the block's first word, which a large class's loser writes with nothing
  carved and its carves after, and where a small class's top summary word sits, nonzero only
  once one of its slots, so one carved, is released. A region carved from was named and keeps
  its tag for good, so the second read gives the carver (`tla/Assign.tla`: `Assign_repair`,
  without the second read, publishes the wrong class; `Assign_repair_recheck` holds;
  `back::tests::published_tag` plays it against the code). The byte is written once (a
  compare-and-swap from zero) and never cleared.

A class never gives a named region up: the only tag change away from `Class(class)` is
`assign` giving back a region it took and did not name, after its swing lost. Such a region
was never published, so no walk has it; a pointer into it was never handed out. The tags
cannot be reached any other way: `back::tags` keeps the table and the published bytes private
and offers a class for a region number only through the published byte (`tla/Assign.tla`
checks the protocol, and `Assign_table` shows a walk that read the table instead acting on a
region another class or a run then had).

The reviewed finding at 0fd0853 (the walks read the table, and could act on a region given
back with the wrong class's layout, `back::tests::published_tag`) is closed by the published
tag, and the finding of the review of 1f52647 (the repair's stale read) by its second read.

## BACK-HOST-01: `&*at` in `ClassHost::meta`

**Preconditions of dereferencing `at` for a shared reference.** `at` is non-null and aligned for
`Atomic`, points to a live, initialized `Atomic` for the lifetime of the returned reference (the
borrow of `self`), its provenance covers the word, and nothing accesses that memory in a way
that conflicts with a shared reference.

**Proof.**

1. *In bounds.* `at = metadata(region) + 8 * word`, made by `Space::metadata_at(region, 8 *
   word)`. Callers pass only word numbers that the
   layout names: `tree_word(level, index)` with `index` below that level's count, for a
   large class its header words, 0 to `HEADER_WORDS - 1` (`header_word()` and the word after
   it, `grow` and `hints`; the hold's stamp, `hold_word()`; and the hold's walk by slot ages,
   `age_word()`), which `layouts_fit` places before the first summary word,
   `clean_word(w)` with `w` below the clean count, `zero_word(w)`
   with `w` below the leaf count, and `size_word(slot)` with `slot` below the slot count (a
   slot `Plan::slot_at` accepted). The layout a host holds is the one worked out: a layout read
   back from the class's memo equals `Layout::new` (Kani, `layouts_come_back_from_the_memo`).
   Kani's `layouts_fit_the_used_geometries` and
   `layouts_fit_every_geometry` prove that all of these sections lie inside
   `total_words * 8 <= metadata_size`. The indexes are below their counts because each comes
   from a slot below the region's slot count (leaf and zero words: `slot / 64`; clean words: a
   page of a slot below the watermark) or from a summary bit that a walk from such a word set
   (summary and leaf words reached in a descent). `Space::metadata(region)` is the region's
   block, `metadata_size` bytes inside the reservation (`space::verify`,
   `plans_keep_their_parts_apart`). `Space::metadata_at` checks the region
   against the plan, as `Space::metadata` does, and adds the region's offset
   (`region << metadata_shift`) and then `offset` to the first block's address, which
   `Space::new` works out once from the plan (`Plan::metadata_start`, the same sum
   `Plan::metadata_offset` makes). The additions are wrapping, but with the region checked and
   the word inside the block they stay inside the reservation, so they never wrap, and the
   result is exactly the address `Space::metadata(region)` plus the offset gives.
   `wrapping_byte_add` keeps the provenance of the first block's pointer, which is the
   reservation's.
2. *Non-null and aligned.* The address lies inside the reservation, which does not contain
   zero. The metadata block starts on a page boundary and the offset is a multiple of 8.
   `Atomic` is `#[repr(transparent)]` over `AtomicU64`, alignment 8.
3. *Accessible.* The word is committed before any thread forms a reference to it:
   - The fixed part is committed by `assign` before its compare-and-swap puts this class's
     tag on the region: for a small class from the block's start to the start of the leaf
     bitmap, which comes last; for a large class the whole block, header and leaf bitmap
     included (`Layout::fixed_commit`, proved in `layouts_fit`). So any thread that learns the region through the table, the published tag, the
     carving word or the summary tree finds it committed: the OS has finished the commit before
     that thread's acquire of the tag, byte or word, and commit is never undone.
   - A leaf word is touched only for a slot below the watermark, or in a descent through set
     summary bits. A carve commits the leaf words of every slot starting below the new commit
     frontier (`Layout::leaf_commit`, proved in `leaf_commits_cover_the_committed_slots`) before
     its compare-and-swap publishes that frontier, and the watermark never passes the frontier
     (both are in the region's one `Grow` word, reached through `grow`: for a small class it
     is kept outside the block in `State::headers` and needs no `unsafe`; for a large class it
     is word 0 of the block, in the fixed part committed before the region is tagged, and
     reached through `meta` as any other word of it; a carve advances the watermark only when
     the frontier covers the new slots). A large class's leaf bitmap is in that fixed part too,
     so every leaf word of a large class is committed whatever the watermark. A small class's
     slots start at its origin (`Plan::slot_origin`, whole pages): the pages below it hold no
     slot and are never committed, so the leaf words a frontier covers start with the first
     slot past the origin, and the harness, run on the layouts with their origins, covers
     that. `validate` checks `slot < carved` before `release`; `purge_unit` looks only at
     slots below the watermark (`unit_below`); a descent reaches a
     leaf word only through a summary bit, and a summary bit is set only by `tree::up`, which
     starts at a leaf word that was just freed into, hence below the watermark.
4. *Live.* The reservation is published by the `SpaceCell` that `State` owns, and only that
   cell's `Drop` unmaps it; the `Space` in the host borrows the cell, so the memory outlives
   the reference.
5. *No conflicting access.* All access to metadata words is through `Atomic`, whose methods
   are atomic operations; nothing writes this memory non-atomically. The OS zeroed it before
   the first commit, which is a valid `AtomicU64`.

## BACK-HOST-03: `slice::from_raw_parts(first, count)` in `ClassHost::tree_words`

**Preconditions of `slice::from_raw_parts`.** `first` is non-null and aligned for `Atomic`;
the `count` words from it are one allocated object, each a live, initialized `Atomic`, for the
lifetime of the slice (the borrow of `self`); `count * 8` is at most `isize::MAX`; and nothing
accesses that memory in a way that conflicts with a shared reference.

**Proof.** The slice is BACK-HOST-01's word references, made for words `tree_word(level, 0)` to
`tree_word(level, count - 1)` at once, so each point of that proof applies to each word:

1. *In bounds, one object.* `first = metadata(region) + 8 * tree_word(level, 0)`, the start of
   that level of the tree, and `level` is checked against `MAX_LEVELS` and `count` against
   `level_words(level)`, the level's length, with an abort before the slice is made. Each level
   lies inside the region's metadata block (BACK-HOST-01, point 1), which is part of the one
   reservation mapping, so the words are contiguous in one allocation and `count * 8` is far
   below `isize::MAX`. With `count` zero the slice is empty, and `first` is still non-null and
   aligned.
2. *Aligned.* As BACK-HOST-01, point 2.
3. *Accessible.* The caller's promise, as `Host::tree_words` states it.
   - A summary level (`level >= 1`) is in the fixed part of the block, committed before the
     region carries the class's tag (BACK-HOST-01, point 3). The caller,
     `Release::summary_levels`, asks for each level from 1 up to the height of the region's
     own tree and for no level above it, and for exactly `level_words(level)` words of each,
     so every word it asks for exists in the layout.
   - The leaf bitmap (`level == 0`): `count` covers only slots below a watermark the caller
     read for this region. The one caller, a launder's scattered path (`release_words` in
     `back/class.rs`, through `Release::leaves`), passes `limit.div_ceil(64)` with `limit` the
     watermark `Checker::meet` read from the region's `Grow` word, at most the slot count. A
     carve commits the leaf words of every slot below the new commit frontier before it
     publishes it, the watermark never passes the frontier, and commit is never undone
     (BACK-HOST-01, point 3), so every word of the slice is committed.

   A slice reference asserts that its whole extent is dereferenceable, which is why it may not
   reach past those words.
4. *Live.* As BACK-HOST-01, point 4.
5. *No conflicting access.* As BACK-HOST-01, point 5: every word is accessed only through
   `Atomic`'s atomic operations, by every thread.

## BACK-HOST-04: `self.space.resident(...)` in `ClassHost::resident`

**Proof.** `ClassHost::resident` is an `unsafe fn` whose `# Safety` is exactly
`Space::resident`'s: no thread writes the range during the call. It forwards the range unchanged
(the region's base plus the offset); `Space::resident` checks it (RESERVATION-06). The caller's
obligation is discharged at its one call site, BACK-REGION-04.

## BACK-HOST-02: `self.space.purge(...)` in `ClassHost::purge`

**Proof.** `ClassHost::purge` is an `unsafe fn` whose `# Safety` is exactly `Space::purge`'s:
nothing touches the range during the call and nothing in it is needed. It forwards the range
unchanged (the region's base plus the offset). The range checks are `Space::purge`'s own: it
aborts on a range outside the reservation or not whole pages. The caller's obligations are
discharged at the call sites: BACK-REGION-01, BACK-REGION-02 and BACK-REGION-03.

**Machine checks.** The simulator records which pages are committed, and fails a commit,
purge or unmap of a page that was written before it was committed. The back end's `SimOs`
tests (`back::tests`) run every access path on it, natively and under Miri with strict
provenance, with fault injection (including calls that fail partway) and with four threads.
`the_leaf_bitmap_is_committed_before_it_is_written` fails if a carve skips the leaf commit
(checked by deleting it). The protocol scenarios in `back::models` run on a test host that does
not track commits; they check the protocols, not this entry. Kani proves the layout's sections
fit the block (`layouts_fit_*`) and the leaf commit covers the carved slots
(`leaf_commits_cover_the_committed_slots`); a Kani run of the protocols on a heap does not fit
the memory cap, so the access paths are tested, not proved.

Reviewed: 8cbb15e
