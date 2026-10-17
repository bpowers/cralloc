# Safety ledger: `crates/asmalloc/src/os/sim/memory.rs`

The simulated OSes' host allocation, shared by `SimOs` and `ProofOs`. Test infrastructure, but
the allocator's proofs and Miri runs rely on it, so it is held to the same rule.

**Machine checks for every entry below.**

- Kani `os::sim::verify::a_tiny_heap_starts_and_its_memory_works` (in the gate) runs allocation,
  zeroing and free through `ProofOs` for every byte offset of a page, and
  `os::sim::verify_slow::the_simulator_keeps_the_zero_promises` does the same through `SimOs`.
  Kani checks every access.
- Miri with `-Zmiri-strict-provenance` (`scripts/miri`) runs all the simulator and `Space` unit
  tests on the `Standard` shape, including threads reserving at once.

The pen-and-paper part is the generalization to every shape and capacity.

## SIM-MEMORY-01: `unsafe impl Send for Memory` and `unsafe impl Sync for Memory`

**Proof.** `Memory` is a `NonNull<u8>` and a `Layout`. It owns its allocation and holds no
references, no thread-affine resources and no interior mutability. Sharing it exposes the base
address, offset arithmetic, and the unsafe `zero` and `is_zero`, whose contracts put
synchronization on the caller. Moving it moves ownership of the allocation, which the global
allocator frees from any thread.

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
of it. `u8` needs no alignment and every value is valid. The callers establish the bound by
checking the range first: `SimOs::checked_range`, `ProofOs::checked` and the recorded wipe
range. They establish exclusivity from their own `Os` contracts (SIM-OS-02, PROOF-01).

## SIM-MEMORY-04: `dealloc` in `Drop for Memory`

**Proof.** `base` and `layout` are exactly what `alloc_zeroed` returned and was given, and drop
runs once. Every `Space` borrows the OS it came from (and `SpaceCell` owns it), so no `Space`
and no safe call through one exists after the drop. Raw pointers copied out of a `Space` may
outlive it, but dereferencing them is unsafe, and the `Os` contract makes reservations valid only
until the value that made them is dropped.

## SIM-MEMORY-05: `from_raw_parts` in `Memory::is_zero`

**Preconditions** (the function's `# Safety`). `offset + len <= self.len()`, and no thread
writes those bytes during the call.

**Proof.** The slice lies within the allocation for the same reason as in SIM-MEMORY-03, it is
initialized (the allocation was zeroed and is only ever written with bytes), and `u8` needs no
alignment. Readers may share it; the caller rules out writers for the slice's lifetime, which
ends with the call. The callers:

- `check_page` reads one page that is not committed, which no correct caller writes; catching
  a buggy writer is the point. Being uncommitted when the check began is not enough, because a
  racing `commit` of the same page could make it writable and let its caller write while the
  check reads (Miri found exactly that in a depot test). So `purge` and `unmap` check only under
  their contracts, which exclude concurrent access, and `commit` checks a page only after
  claiming it with `CHECKING` (`Pages::begin_commit`): a racing commit waits for the claim to
  end, and the page becomes writable only when the check is done.
- `is_purged` and `resident_bytes` are test queries documented "call while nothing writes".

Machine checks: Miri over the simulator's unit tests and the depot's threaded tests, which run
all three callers. Kani does not reach this block: the check is skipped under Kani, because word
reads of memory a harness wrote at a symbolic byte offset cost CBMC minutes.

**Review.** 2026-10-17, fresh adversarial reviewer: SIM-MEMORY-01 to 04 accepted. SIM-MEMORY-05
is new and needs the reviewer's check.
