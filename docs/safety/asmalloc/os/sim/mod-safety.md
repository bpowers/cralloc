# Safety ledger: `crates/asmalloc/src/os/sim/mod.rs`

The simulated OS's `Os` implementation. Machine checks are as for `memory-safety.md`: the slow
Kani harness on the `Tiny` shape, and Miri over the unit tests on the `Standard` shape.

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
  other allocation: they lie inside `memory`, which this value owns.
- Commit keeps contents (it only sets flags). Purge zeroes exactly its range. An injected
  failure returns before any effect.
- Values are not interchangeable, so the trait's alternative applies. A range from another
  value's memory lies outside this value's allocation, because allocations are disjoint, so
  `checked_range` panics before touching anything.
- `abort` panics, so it does not return.

## SIM-OS-02: `self.memory.zero(...)` in `purge` and `unmap`

**Preconditions of `Memory::zero`.** The range is inside the memory, and no other thread
touches it during the call.

**Proof.** `checked_range` returned, so `offset_of(start)` and `len` describe whole pages
inside the memory; otherwise it panicked. The `purge` contract says no thread reads or writes
the range during the call. The `unmap` contract says nothing touches it after the call begins.
So the write races with nothing.

**Review.** Pending: needs an adversarial review by a fresh agent.
