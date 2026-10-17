# Safety ledger: `crates/asmalloc/src/os/sim/mod.rs`

The simulated OS's `Os` implementation. Machine checks are as for `memory-safety.md`: the slow
Kani harness on the `Tiny` shape, and Miri over the unit tests on the `Standard` shape
(`scripts/miri`).

## SIM-OS-01: `unsafe impl<S: Shape> Os for SimOs<S>`

The comment above the impl lists how each promise of the trait holds. The load-bearing
invariant is the zero promise:

**Invariant Z.** Every byte of simulated memory that is not in a reserved page is zero.

- Initially, `Memory::new` zero-fills the allocation.
- `reserve` changes no bytes. It moves pages from unreserved to reserved by compare-and-swap
  from state 0 (`Pages::claim`), and rolls back its own claims on a conflict, so two reservations
  never share a page.
- `unmap` zeroes its whole range, whether or not it was committed, before releasing the pages
  with release stores. A later `claim` uses acquire compare-and-swaps that read those stores, so
  the next owner sees the zeroes.
- No other call releases pages.

So a reservation starts out zero, which is the trait's promise that reserved memory reads as
zero once committed. That holds even if a buggy caller wrote to pages it never committed. The
other promises:

- Reservations are aligned: candidate offsets step through aligned addresses.
- Reservations are disjoint from each other: per-page claims. They are disjoint from every
  other allocation: they lie inside `memory`, which this value owns until it is dropped.
- *Valid until unmapped or dropped*: the memory is freed only in `Drop`.
- *A range in one live reservation of this value.* `checked_range` panics before touching
  anything otherwise:
  - The range is outside this value's memory.
  - It contains a page that is not reserved.
  - It spans two reservations. The first page of each reservation carries `START`. `unmap`
    moves `START` to the remaining tail when it removes a reservation's head, as munmap does.
    It decides this before releasing its range, with a compare-and-swap that requires the page
    still to be reserved, so a concurrent reserve or unmap cannot be confused with the tail.
    This is a correctness matter for tests, not memory safety: a wrong flag can only make a
    later check panic, or leave a page unclaimable.
- Commit keeps contents (it only sets flags). Purge zeroes exactly its range. An injected
  failure returns before any effect.
- `abort` panics, so it does not return.

## SIM-OS-02: `self.memory.zero(...)` in `purge` and `unmap`

**Preconditions of `Memory::zero`.** The range is inside the memory, and no other thread
touches it during the call.

**Proof.** `checked_range` returned, so `offset_of(start)` and `len` describe whole pages
inside the memory; otherwise it panicked. The `purge` contract says no thread reads or writes
the range during the call. The `unmap` contract says nothing touches it after the call begins.
So the write races with nothing.

**Review.** 2026-10-17, fresh adversarial reviewer: SIM-OS-02 accepted. SIM-OS-01 was not: a
`Space` could outlive its `SimOs`, and with address reuse a stale range could pass the check;
also a range spanning two adjacent reservations was accepted. Both are fixed: a `Space` now
borrows its OS, so it cannot outlive it, and the `START` flag detects spanning ranges. The fixes
await the same reviewer's check.
