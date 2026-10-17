# Safety ledger: `crates/asmalloc/src/heap/sim.rs`

`SimHeap` is a heap over the simulated OS with twins on its own slabs; a real thread borrows a
simulated thread (a lease) for the length of one call.

## SIM-01: `unsafe impl Routines for SimRoutines`

**Proof.** Every method builds `Twin::new(self.slabs, self.processor)` and calls the same
method, so each obligation is TWIN-04's (`front/twin-safety.md`), for the machine
`Processor`. `Processor` is a `SimThread` (whose `Machine` contract the simulator keeps:
it aborts a sequence when its CPU changes, when a fence hits its CPU, or when another
simulated thread starts a sequence on its CPU; and its fence returns only once no sequence it
aborted can still write a magazine or commit, which matters here because the real threads run
freely) or an unregistered thread (`cpu_id` -2), which
touches no slab. A lease gives a simulated thread to one real thread at a time (the flag is
claimed by compare-and-swap and released only by the `Lease` that claimed it), so one simulated
thread never runs two sequences at once.

## SIM-02: `unsafe impl GlobalAlloc` and `Allocator` for `SimHeap` and `SimAsmalloc`

**Proof.** As GLOBAL-01 and GLOBAL-03 (`heap/global-safety.md`), for this heap and its routines.
`SimAsmalloc` forwards every call to the one shared `SimHeap`, so every handle reaches the
same heap.

## SIM-03: the calls to the heap's `unsafe fn`s inside `SimHeap::with`

**Precondition** (H): the routines' slab array is used only with this heap.

**Proof.** `SimRoutines::slabs` is `self.slabs`, a box the `SimHeap` owns and never hands out,
paired with `self.heap` for the life of the value. The other preconditions are the callers'.

## SIM-04: `Box::from_raw(mine)` and `&*current` in `SimAsmalloc::heap`

**Proof.** `mine` came from `Box::into_raw` just above and, when the compare-and-swap fails,
was never published, so this thread is its only owner and frees it once. `current` is either
`mine` after a successful publish or the winner another thread published; a published heap is
never freed or replaced (the static is only ever set from null), so the reference is valid for
`'static`. `SimHeap` is `Sync` (atomics and immutable fields), so sharing it is sound.
