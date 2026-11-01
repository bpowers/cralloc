# Safety ledger: `crates/cralloc/src/space/reservation.rs`

`Space` wraps the OS's memory calls so that code above it commits and advises without `unsafe`.
Kani does not cover these blocks. It does cover the address arithmetic they depend on, in
`space/verify.rs` and `verify_slow.rs`.

**The `Space` invariant.** The `plan.size()` bytes at `base` are one reservation that `os` (the
value the `Space` borrows) returned and that has not been unmapped, and `base` is aligned to the
plan's region size, which is the size the reservation got (regions shrink, keeping their number,
when the OS refuses the largest). A `Space` stores the plan's region shift and rebuilds the plan
as `Plan::new(O::GEOMETRY, shift)` (`Space::plan`); every plan it is built from is
`O::GEOMETRY.plan(shift)`, which is that same value, so "the plan" below is one plan whichever
way it is read.

Three places construct a `Space`:

- `Space::reserve(os)` and `SpaceCell::get_or_reserve` each build one from the pointer
  `os.reserve(plan.size(), plan.region_size())` returned (the cell's own OS for the cell), for
  the plan of the region size it was asking for. By the `Os` contract, that is a live
  reservation of that length and alignment. The cell's count of reservers in flight decides
  only which size to ask for next; it plays no part in what a `Space` describes.
- `SpaceCell::decode` rebuilds one from the cell's word, borrowing the cell's own OS.
  - Only `SpaceCell::publish`, called from `get_or_reserve`, writes the word, and only with
    `encode` of the `Space` just built from the cell's own OS.
  - `encode` stores `pack(base, region_shift)`, and `decode` reads `unpack(word)` and rebuilds
    the plan from the geometry and the shift. Kani's `published_words_round_trip_*` proves
    `unpack(pack(base, shift)) == (base, shift)` for every region-aligned non-zero base and every
    shift a plan can have. So `decode` gives back the base and plan that were stored.

The borrow ties every `Space` to an OS value that is still alive. The `Os` contract keeps a
reservation valid until it is unmapped or that value is dropped, so liveness reduces to "not
unmapped". The invariant ends only through the unsafe `Space::unmap`, whose callers promise that
no copy is used afterwards and that it is not a published space.

## RESERVATION-01: the range calls in `commit`, `advise_huge`, `split_huge` and `wipe_on_fork`

**Preconditions of `Os::commit`, `Os::advise_huge`, `Os::split_huge` and `Os::wipe_on_fork`.**
The range is page-aligned, a whole number of pages, not empty, and inside one live reservation
made by the same `Os` value. None of the four changes contents in this process.

**Proof.** `check` returned, so `is_callable_range` held:

- `len != 0`;
- `offset = start - base` is below `plan.size()`, and `len <= plan.size() - offset`, so the
  range lies inside the reservation, with no overflow given the form of the subtraction;
- `offset | len` has no bits below the page size. The base is region-aligned and so
  page-aligned, so the start is page-aligned and the length is whole pages.

The call goes to `self.os`, the value that made the reservation, which is live by the
invariant. `check` calls `os.abort` on failure, which does not return.

## RESERVATION-02: `os.purge` in `Space::purge`

**Proof.** The range conditions are as for RESERVATION-01. The extra `Os::purge` conditions (no
thread touches the range during the call, nothing in it is needed) are this function's own
`# Safety` conditions, forwarded unchanged.

## RESERVATION-06: `os.resident` in `Space::resident`

**Proof.** The range conditions are as for RESERVATION-01. The extra `Os::resident` condition
(no thread writes the range during the call) is this function's own `# Safety` condition,
forwarded unchanged.

## RESERVATION-03: `os.unmap` in `Space::unmap`

**Proof.** The range is the whole reservation, `base` and `plan.size()`. The size is the end of
the process page, which starts page-aligned and is one page (`plans_keep_their_parts_apart`).
So the range is whole pages and not empty.
It is live by the invariant, and the caller promises nothing uses it afterwards, which is the
`Os::unmap` condition.

## RESERVATION-04: `mine.unmap()` in `SpaceCell::publish`

**Proof.** The compare-and-swap failed, so `mine`'s word was never stored. No other thread
learned its address, and `mine` was built by this call of `get_or_reserve`, which passed it
here and does not use it again. That meets
RESERVATION-03's precondition. A failed unmap leaves an unused reservation behind, which costs
address space only.

## RESERVATION-05: `space.unmap()` in `Drop for SpaceCell`

**Proof.** `drop` has `&mut self`, so no `Space` borrowed from the cell is alive. None can be
made afterwards, because the cell is going away. The reservation is the cell's own published
space, decoded as in the invariant section, so it is live (the cell's OS has not been dropped
yet; fields drop after `drop` returns). Nothing uses it again, which meets RESERVATION-03's
precondition. A cell in a static is never dropped. This is the one place a published space is
unmapped, and `Space::unmap`'s documentation says so.

## Pointers a `Space` hands out (no unsafe blocks)

`region`, `slot`, `metadata`, `magazines`, `process_page` and `pointer` compute pointers with
`map_addr` from `base`, so each carries the reservation's provenance; forming them is safe.
(`metadata` starts from the first metadata block's address, which `Space::new` forms once from
`base` the same way, `base + Plan::metadata_start`.)
`region` and `metadata` abort unless the id is an object region, `slot` aborts unless the slot
lies wholly inside its region (`Plan::slot_fits`, proved against the slot arithmetic, the
class's origin included, by `slots_lie_inside_their_region_and_are_aligned_*`), and `pointer` returns `None` outside the
reservation. So every pointer is inside the reservation, at the place its name says. Whether
the memory there may be read or written (committed, owned by the caller) is the obligation of
whoever dereferences it, in their own ledger.

**Machine checks.** The `Space` unit tests, natively and under Miri with strict provenance
(`scripts/miri`):

- `commit_makes_memory_writable_and_purge_zeroes_it`;
- `ranges_outside_the_reservation_are_refused` and `a_bad_commit_aborts`;
- `the_cell_publishes_one_space`;
- `racing_threads_publish_one_space_and_losers_unmap`, which checks that only the winner's
  reservation remains;
- `a_failed_reservation_publishes_nothing`;
- `a_failure_while_another_reserves_is_retried_at_the_same_size`,
  `a_failure_with_no_other_reserver_halves_at_once`,
  `a_stale_count_costs_bounded_patience_and_then_halves` and
  `threads_racing_for_room_for_one_heap_all_get_the_largest`, for the count of reservers.

On the real kernel, `os::linux::kernel::tests::the_design_reservation_is_available`.

Reviewed: 502257d