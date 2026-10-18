# Safety ledger: `crates/asmalloc/src/back/host.rs`

`ClassHost` is the protocols' view of one class of the real heap. It turns metadata word
numbers into references into the reservation, and forwards purges to `Space`.

**The `ClassHost` invariant.** `layout` is `Layout::new(space.plan().geometry(), class)` for the
host's class, and every `RegionId` a caller passes is a region the region table tags with that
class. `Back::host` is the only constructor, and it builds the layout that way. Region ids
reach a host only from:

- the class's carving word, which holds only a region this class's `assign` took from the table
  with a compare-and-swap from `Unassigned` to `Class(class)`;
- the class's summary tree (`tree::child` of a class word), whose bits are set only by the walk
  up from a leaf word of one of the class's regions;
- `Plan::locate` of a pointer whose table tag `validate` or `purgeable` has just checked is this
  class.

A class never gives a region up. The only tag change away from `Class(class)` is `assign`
reverting a region it took but did not publish, which no other thread has named.

## BACK-HOST-01: `at.as_ref()` in `ClassHost::meta`

**Preconditions of `NonNull::as_ref`.** `at` is aligned for `Atomic`, points to a live,
initialized `Atomic` for the lifetime of the returned reference (the borrow of `self`), and
nothing accesses that memory in a way that conflicts with a shared reference.

**Proof.**

1. *In bounds.* `at = metadata(region) + 8 * word`. Callers pass only word numbers that the
   layout names: `tree_word(level, index)` with `index` below that level's count, for a
   large class `header_word()` and the word after it (`grow` and `hints`, words 0 and 1),
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
   `plans_keep_objects_metadata_and_magazines_apart`); it checks the region against the plan
   and adds the region's offset to the first block's address, which `Space::new` works out
   once from the plan (`Plan::metadata_start`, the same sum `Plan::metadata_offset` makes).
   `ClassHost::block` keeps the last block it got from `Space::metadata`, with the region it
   asked for, and hands it out again only for that same region, so `metadata(region)` is
   always the block `Space::metadata` gave for `region`. The kept pair lives in the
   `ClassHost`, which belongs to one operation on one thread's stack, and the space it came
   from is the host's own, which never changes.
2. *Aligned.* The metadata block starts on a page boundary and the offset is a multiple of 8.
   `Atomic` is `#[repr(transparent)]` over `AtomicU64`, alignment 8.
3. *Accessible.* The word is committed before any thread forms a reference to it:
   - The fixed part is committed by `assign` before its compare-and-swap puts this class's
     tag on the region: for a small class from the block's start to the start of the leaf
     bitmap, which comes last; for a large class the whole block, header and leaf bitmap
     included (`Layout::fixed_commit`, proved in `layouts_fit`). So any thread that learns the region through the table, the carving word or
     the summary tree finds it committed: the OS has finished the commit before that thread's
     acquire of the tag or word, and commit is never undone.
   - A leaf word is touched only for a slot below the watermark, or in a descent through set
     summary bits. A carve commits the leaf words of every slot starting below the new commit
     frontier (`Layout::leaf_commit`, proved in `leaf_commits_cover_the_committed_slots`) before
     its compare-and-swap publishes that frontier, and the watermark never passes the frontier
     (both are in the region's one `Grow` word, kept outside the block in `State::headers`
     and reached through `grow`, which needs no `unsafe`; a carve advances the watermark only
     when the
     frontier covers the new slots). `validate` checks `slot < carved` before `release`;
     `purge_unit` looks only at slots below the watermark (`unit_below`); a descent reaches a
     leaf word only through a summary bit, and a summary bit is set only by `tree::up`, which
     starts at a leaf word that was just freed into, hence below the watermark.
4. *Live.* The reservation is published by the `SpaceCell` that `State` owns, and only that
   cell's `Drop` unmaps it; the `Space` in the host borrows the cell, so the memory outlives
   the reference.
5. *No conflicting access.* All access to metadata words is through `Atomic`, whose methods
   are atomic operations; nothing writes this memory non-atomically. The OS zeroed it before
   the first commit, which is a valid `AtomicU64`.

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

Reviewed: 415b07e
