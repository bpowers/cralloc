# Safety ledger: `crates/asmalloc/src/os/sim/memory.rs`

The simulated OS's host allocation. Test infrastructure, but the allocator's proofs and Miri
runs rely on it, so it is held to the same rule.

**Machine checks for every entry below.**

- Kani `os::sim::verify_slow::the_simulator_keeps_the_zero_promises` runs allocation, zeroing
  and free on the `Tiny` shape for every byte offset and value. Kani checks every access.
- Miri with `-Zmiri-strict-provenance` runs all the simulator and `Space` unit tests on the
  `Standard` shape, including eight threads reserving at once.

The pen-and-paper part is the generalization to every shape and capacity.

## SIM-MEMORY-01: `unsafe impl Send for Memory` and `unsafe impl Sync for Memory`

**Proof.** `Memory` is a `NonNull<u8>` and a `Layout`. It owns its allocation and holds no
references, no thread-affine resources and no interior mutability. Sharing it exposes the base
address, offset arithmetic, and the unsafe `zero`, whose contract puts synchronization on the
caller. Moving it moves ownership of the allocation, which the global allocator frees from any
thread.

## SIM-MEMORY-02: `alloc_zeroed` in `Memory::new`

**Precondition.** The layout's size is not zero.

**Proof.** The preceding `assert!(len > 0)` guarantees it, and `Layout::from_size_align` checked
the alignment. A null result goes to `handle_alloc_error`, which does not return, so `base` is a
valid, zeroed allocation of `layout`.

## SIM-MEMORY-03: `write_bytes` in `Memory::zero`

**Preconditions** (the function's `# Safety`). `offset + len <= self.len()`, and no other thread
reads or writes those bytes during the call.

**Proof.** `base.as_ptr().add(offset)` stays within the allocation because
`offset <= self.len()`, and writing `len` bytes from there stays within it because
`len <= self.len() - offset`. `base` came from the allocator, so it carries provenance for all
of it. `u8` needs no alignment and every value is valid. The callers, `SimOs::purge` and
`SimOs::unmap`, establish the bound with `checked_range`, which panics unless the range is
inside the memory. They establish exclusivity from their own `Os` contracts (SIM-OS-02).

## SIM-MEMORY-04: `dealloc` in `Drop for Memory`

**Proof.** `base` and `layout` are exactly what `alloc_zeroed` returned and was given, and drop
runs once. Raw pointers the simulator handed out may outlive it, but dereferencing them is
unsafe and requires a live reservation. Every safe path into the memory (`Space::commit` and
the rest) borrows the `SimOs` for the call, so it cannot run after the drop.

**Review.** Pending: needs an adversarial review by a fresh agent.
