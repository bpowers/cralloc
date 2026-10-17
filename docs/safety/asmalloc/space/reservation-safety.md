# Safety ledger: `crates/asmalloc/src/space/reservation.rs`

`Space` wraps the OS's memory calls so that code above it commits and advises without `unsafe`.
Kani does not cover these blocks. It does cover the address arithmetic they depend on, in
`space/verify.rs` and `verify_slow.rs`.

**The `Space` invariant.** The `plan.size()` bytes at `base` are one reservation that `os` (the
value the `Space` borrows) returned and that has not been unmapped, and `base` is aligned to the
region size.

Two places construct a `Space`:

- `Space::reserve(os)` builds one from the pointer `os.reserve(plan.size(), region_size)`
  returned. By the `Os` contract, that is a live reservation of that length and alignment.
- `SpaceCell::decode` rebuilds one from the cell's word, borrowing the cell's own OS.
  - Only `SpaceCell::get_or_reserve` writes the word, and only with `encode` of a `Space` that
    `Space::reserve(&self.os)` returned.
  - `encode` stores `pack(base, objects)`, and `decode` reads `unpack(word)`. Kani's
    `published_words_round_trip_*` proves `unpack(pack(base, objects)) == (base, objects)` for
    every region-aligned non-zero base and valid count. So `decode` gives back the base and plan
    that were stored.

The borrow ties every `Space` to an OS value that is still alive. The `Os` contract keeps a
reservation valid until it is unmapped or that value is dropped, so liveness reduces to "not
unmapped". The invariant ends only through the unsafe `Space::unmap`, whose callers promise that
no copy is used afterwards and that it is not a published space.

## RESERVATION-01: `os.commit` in `Space::commit`, and `os.advise_huge` in `Space::advise_huge`

**Preconditions of `Os::commit` and `Os::advise_huge`.** The range is page-aligned, a whole
number of pages, not empty, and inside one live reservation made by the same `Os` value.

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

## RESERVATION-03: `os.unmap` in `Space::unmap`

**Proof.** The range is the whole reservation, `base` and `plan.size()`. The size is the end of
the magazine area, which starts page-aligned and is whole pages
(`plans_keep_objects_metadata_and_magazines_apart`). So the range is whole pages and not empty.
It is live by the invariant, and the caller promises nothing uses it afterwards, which is the
`Os::unmap` condition.

## RESERVATION-04: `mine.unmap()` in `SpaceCell::get_or_reserve`

**Proof.** The compare-and-swap failed, so `mine`'s word was never stored. No other thread
learned its address, and `mine` is a local of this call that is not used again. That meets
RESERVATION-03's precondition. A failed unmap leaves an unused reservation behind, which costs
address space only.

**Machine checks.** The `Space` unit tests, natively and under Miri with strict provenance
(`scripts/miri`):

- `commit_makes_memory_writable_and_purge_zeroes_it`;
- `ranges_outside_the_reservation_are_refused` and `a_bad_commit_aborts`;
- `the_cell_publishes_one_space`;
- `racing_threads_publish_one_space_and_losers_unmap`, which checks that only the winner's
  reservation remains;
- `a_failed_reservation_publishes_nothing`.

On the real kernel, `os::linux::kernel::tests::the_design_reservation_is_available`.

**Review.** 2026-10-17, fresh adversarial reviewer: RESERVATION-01 to 03 accepted as
range-check proofs. The invariant section was not accepted: a `Space` could outlive its
`SimOs`, and the Kani claim named `decode(encode)` where only `pack`/`unpack` is proved. Both are
fixed above (the `Space` now borrows its OS, and the cell owns its OS and alone writes the
word). The fix and RESERVATION-04 are new and await the same reviewer's check.
