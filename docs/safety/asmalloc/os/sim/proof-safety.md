# Safety ledger: `crates/asmalloc/src/os/sim/proof.rs`

`ProofOs`, the simulated OS for Kani: one reservation at a time, at the start of its memory.
Machine checks: Kani `os::sim::verify::a_tiny_heap_starts_and_its_memory_works` (in the gate)
starts a `Tiny` heap on it, writes and purges a page, and drops it, with every access checked;
the unit tests in `proof.rs` run natively and under Miri.

## PROOF-01: `unsafe impl<S: Shape> Os for ProofOs<S>`

**Invariant Z.** Every byte of memory outside the reservation is zero.

- `Memory::new` zero-fills the allocation.
- `reserve` changes no bytes; it claims the one reservation, starting at offset 0, by a
  compare-and-swap of `reserved` from zero, so there is never more than one.
- `unmap` must cover the whole reservation (it panics otherwise), zeroes it, and only then
  stores zero to `reserved` with release ordering, which the next `reserve`'s acquire reads.

So a reservation starts out zero. The other promises:

- The reservation is aligned as asked: it starts at the allocation's base, which is aligned to
  the shape's largest region size, and `reserve` refuses a larger alignment.
- It is disjoint from every other allocation: it lies inside `memory`, which this value owns until
  it is dropped. It carries the allocation's provenance.
- *A range in the live reservation of this value.* `checked` panics before touching anything if
  the range is not whole pages inside `0..reserved`.
- `commit` and `advise_huge` change nothing. `purge` zeroes its range, or the first half for a
  partial fault. `wipe_on_fork` records one range. A failed call acts on no more than its range.
- `fence` aborts the twin sequences in flight on its CPU, waiting out committing ones.
- `abort` panics, so it does not return.

What it does not promise beyond the trait: it does not detect a write to uncommitted memory. That
is a test aid of `SimOs`, not part of the `Os` contract.

## PROOF-02: `self.memory.zero(...)` in `purge`, `unmap` and `fork`

**Preconditions of `Memory::zero`.** The range is inside the memory, and no other thread
touches it during the call.

**Proof.** In `purge` and `unmap`, `checked` returned, so the range is inside `0..reserved`,
inside the memory; the zeroed part of a partial purge is a prefix of it. In `fork`, the range is
the one `wipe_on_fork` recorded after `checked` accepted it. A reservation never moves, and
`unmap` clears the record, so the range is still inside the memory. Exclusivity comes from the
`purge` and `unmap` contracts and from `fork`'s own `# Safety`.

**Review.** New; needs an adversarial review by a fresh agent.
