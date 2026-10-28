# What makes the allocator's tests slow under Miri

Measured 2026-10-25 to 2026-10-28 on the reference host, through `scripts/heavy` (eight cores
shared with other teammates' builds, and often paused for their measurements), with the nightly
Miri of that week and `-Zmiri-strict-provenance`.

## Summary

- **The cost was the aliasing model, not the tests' loops.** Tests that run in 2 to 20 ms
  natively took over 15 CPU minutes under Miri. With Stacked Borrows off, the same tests
  finished in a minute or two. A `perf` profile of Miri put the time in
  `sb_reborrow` and `visit_freeze_sensitive`: Miri retags a shared reference by visiting every
  interior-mutable range of the pointee, so a reference to a struct of thousands of atomics
  costs thousands of visits each time it is passed. Tree Borrows halves it, no more.
- **What a retag cost, by structure** (2,000 calls of a non-inlined function taking `&T`,
  CPU seconds for all of them, Stacked Borrows):

  | `T` | before | after |
  |---|---|---|
  | `u64` | 1 | 1 |
  | `back::State` | 217 | 13 |
  | `space::RegionTable` | 23 | 1 |
  | `depot::Depot` | not measured (64 domains) | 4 (4 domains) |
  | `front::Slabs<[Slab; 64]>` | 104 | (8 simulated CPUs under Miri) |

  With 64 domains, `depot::rules::collect_and_maintain_empty_every_stack` (2 ms natively)
  ran over 900 CPU seconds; with 4, 22. "After" is with the per-region tables and each class's summary words held in one
  `UnsafeCell` (`cells::Cells`), four cache domains under Miri, and eight simulated CPUs under
  Miri.
- **The simulator's zero check** compared each committed page a word at a time, interpreted;
  a test that commits a whole simulated heap spent nearly all its time there. Comparing byte
  slices compiles to `memcmp`, which Miri runs natively.
- **Then the work itself.** With those fixed, the slowest tests run 25,000 to 300,000 basic
  blocks a second under Miri, and the ones still over ten CPU minutes do that much work: the
  depot's rule tests run hundreds of rounds of slow paths, the back end's fill whole heaps.
  They want their counts scaled under Miri (`scale::under_miri`), test by test.

## Runs

`scripts/miri`, six tests at a time, 600 CPU seconds each:

| Code | Passed | Over the limit | Failed | CPU | Wall |
|---|---|---|---|---|---|
| main at 3b5a3f5, one process for all tests | (did not finish in 4 h) | | | | |
| first changes (4 domains, memcmp) | 337 of 388 | 49 | 2 (leaks) | 12.7 h | 6.3 h |
| per-region tables in `Cells` | 351 of 393 | 42 | 0 | 11.7 h | 5.3 h |

A rerun of the slowest 47 after the summary words went into `Cells` and the simulated CPUs
became eight under Miri: 17 passed, 26 over the limit, 4 failed (below).

## Open

Four tests of the simulated heap fail under Miri and pass natively, deterministically: under
Miri with preemption off, with weak-memory emulation off, and without isolation alike; and
natively with every Miri-only setting forced (4 domains, 8 simulated CPUs, unguarded simulated
memory). None is undefined behaviour; each is an assertion of the heap's policy:
`a_fresh_heaps_first_objects_of_a_small_class_lie_on_lines_of_their_own` (the first siblings
share a line), `a_class_opens_one_run_at_start_whatever_the_clock_reads` (six runs, not one),
`large_allocations_and_frees_run_the_heartbeat` (objects left parked on the CPUs) and
`review_lean_realloc::a_simulated_heap_gives_back_the_blocks_a_resize_leaves`. None had
finished under Miri before, so whether they ever passed there is not known.
