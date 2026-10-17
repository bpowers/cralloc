# Safety ledger: `crates/asmalloc/src/space/reservation.rs`

`Space` wraps the OS's memory calls so that code above it commits and advises without `unsafe`.
These blocks are not covered by Kani. The address arithmetic they depend on is: see
`space/verify.rs` and `verify_slow.rs`.

**The `Space` invariant.** The `plan.size()` bytes at `base` are one reservation that some
value of `O` returned and that has not been unmapped, and `base` is aligned to the region size.

Only two places construct a `Space`:

- `Space::reserve` builds one from the pointer `os.reserve(plan.size(), region_size)` returned.
  By the `Os` contract that is a live reservation of exactly that length with that alignment.
- `decode`, reached through `SpaceCell::get` and `SpaceCell::publish`, rebuilds one from a word
  that only `publish` stores, and `publish` stores only `encode(space)` of a `Space`. Kani's
  `published_words_round_trip_*` proves that `decode(encode(s))` has `s`'s base and object
  count, and so the same plan.

The invariant ends only through the unsafe `Space::unmap`, whose callers promise that no copy
is used afterwards and that it is not the published space.

## RESERVATION-01: `os.commit` in `Space::commit`, and `os.advise_huge` in `Space::advise_huge`

**Preconditions of `Os::commit` and `Os::advise_huge`.** The range is page-aligned, a whole
number of pages, not empty, and inside one live reservation from this `Os`.

**Proof.** `check` returned, so `is_callable_range` held:

- `len != 0`;
- `offset = start - base` is below `plan.size()`, and `len <= plan.size() - offset`, so the
  range lies inside the reservation, with no overflow because of the subtraction's form;
- `(offset | len)` has no bits below the page size. The base is region-aligned and so
  page-aligned, so the start address is page-aligned, and the length is whole pages.

The reservation is live by the invariant. It may have come from a different value of `O` than
`os`; the trait requires values to be interchangeable, or to detect a foreign range and abort or
panic before touching memory. `check` calls `os.abort` on failure, which does not return.

## RESERVATION-02: `os.purge` in `Space::purge`

**Proof.** The range conditions are as for RESERVATION-01. The extra `Os::purge` conditions (no
thread touches the range during the call, nothing in it is needed) are this function's own
`# Safety` conditions, forwarded unchanged.

## RESERVATION-03: `os.unmap` in `Space::unmap`

**Proof.** The range is the whole reservation: `base` and `plan.size()`. The plan's size is the
end of the magazine area. The magazine area starts page-aligned and is whole pages
(`plans_keep_objects_metadata_and_magazines_apart`), so the size is whole pages and not empty.
It is live by the invariant. The caller promises nothing uses it afterwards, which is the
`Os::unmap` condition.

**Machine checks.** The `Space` unit tests, natively and under Miri with strict provenance:
`commit_makes_memory_writable_and_purge_zeroes_it`, `ranges_outside_the_reservation_are_refused`,
`a_bad_commit_aborts`, `the_cell_publishes_one_space`. Also
`os::linux::kernel::tests::the_design_reservation_is_available` on the real kernel.

**Review.** Pending: needs an adversarial review by a fresh agent.
