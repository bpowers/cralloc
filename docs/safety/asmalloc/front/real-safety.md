# Safety ledger: `crates/asmalloc/src/front/real.rs`

`Real` wraps the routines generated from Lean (`front/generated/`). Each generated function
states, in its `# Safety` section, what the Rust side must guarantee; the Lean proof covers the
rest (the instructions do what the contract says, under any interruption). The obligations
below are those sections, discharged once here for every call in the file.

**The obligations, from the generated `# Safety` sections.**

- (R) glibc set `__rseq_offset` for the calling thread's rseq area.
- (C) The class index is below 512 and the mask (the `MASK` constant inline, `MAG_MASKS[class]`
  out of line) is `8n - 16` for the class's magazines of `n = 2^k` slots, `2 <= k <= 16`.
- (L) `SLABS`, the thread's `rseq_cs` and `cpu_id`, `__rseq_offset` and the mask entry do not
  overlap each other (where one is written), the stack the routine uses, or the end of the
  address space.
- (S) In every slab, the class's hot word is zero, a marker, or a slot `s >= 1` of a magazine of
  the class that nothing else names, with non-null pointers in slots below; every spare head is
  zero, a marker or a stack of magazines linked through slot 0. Every thread and the drainer
  keep this at all times.
- For `slab_stack_push`: the magazine is the caller's and its address a non-zero multiple of 8.

**Why they hold.**

- (R): glibc 2.35 and later set `__rseq_offset` before any user code runs, for every thread,
  whether or not registration succeeded (`docs/research/linux-rseq-contract.md`, "What the
  fields read"); an unregistered thread reads `cpu_id = -2`, which the routines' unsigned
  compare sends to the miss path. The declaration is GLIBC-01 (`os/linux/glibc-safety.md`).
- (C): the class index comes from a `SmallClass`, so it is 1 to `SMALL_COUNT` (44), below
  512. The masks come from `front::MASKS`, built from `SmallClass::magazine().mask()`, and
  `MAG_MASKS` is that same constant; `MagazineSize` keeps `n` a power of two from 4 to 256,
  so `k` is 2 to 8. `by_class!` checks at compile time that its index list is exactly
  `1..=SMALL_COUNT`, so each instance gets its own class's mask. `front::tests` pins the
  generated constants to the crate's.
- (L): `SLABS` and `MAG_MASKS` are distinct statics, `__rseq_offset` is glibc's, and the rseq
  area is in glibc's thread control block; the stack is the thread's own. None of them is at
  the top of the address space (user space ends far below it on x86-64).
- (S): the slab invariant of `front/twin-safety.md`, which every writer of a slab keeps:
  the routines themselves (proved in Lean), the slow paths through `store_if_equal` and
  `stack_push` (SLOW-01, SPARE-01), and the drain. `Real` changes nothing about that argument:
  it is the same specification with the same writers.

## REAL-01: the inlined routines (`slab_pop`, `slab_push`, `current_cpu`)

**Proof.** The calls pass constant class indexes and masks under (C); (R), (L) and (S) hold as
above. `current_cpu` only reads `__rseq_offset` and `cpu_id`.

## REAL-02: the out-of-line routines

`slab_pop` and `slab_push` (in `pop_out_of_line`, `push_out_of_line`), `slab_cas`
(`store_if_equal`), `slab_stack_push` and `slab_stack_pop`.

**Proof.** (R), (L) and (S) as above. For pop and push, (C) with the class from a `SmallClass`
and the mask from `MAG_MASKS`. On a miss they tail-jump to `front::pop_slow` and
`front::push_slow` with their own arguments; those are the heap's slow paths (GLOBAL-04).
`push_out_of_line`'s caller promises the object is a block of the class it owns (its
`# Safety`), so the push hands the heap a valid free. `store_if_equal` and `stack_push` are
`unsafe fn`s of `Routines` whose callers promise the new word keeps (S) and that they hold the
magazine (SLOW-01, SPARE-01); `Magazine::base` is non-zero and aligned to the magazine's size,
at least 32. The offsets come from `Field`, always a slot of the slab.

## REAL-03: `unsafe impl Routines for Real`

**Obligations** (the trait's `# Safety`). Each method behaves as its specification, as one step
that either commits or changes nothing, on the slab of the CPU the thread was on throughout.

**Proof.** Each method is one generated routine whose Lean contract is that specification
(`lean/Asmalloc/Spec/Slab.lean`): pop and push (inline), `slab_cas`, `slab_stack_push`,
`slab_stack_pop`, and `current_cpu` for `cpu`, which is not a sequence and promises nothing
about staying current. The raw results are decoded by `StoreIfEqual::from_raw`,
`StackPush::from_raw` and `StackPop::from_raw`, which follow the contracts' return values
(`NO_CPU` is 2, never a slab word). `slabs()` returns `SLABS`, the array the routines address
by symbol, so the trait's view and the assembly's are the same memory.

**Provenance.** The routines take and return addresses as integers. `push` exposes the object's
provenance; `pop` takes the exposed provenance back. Every object address lies in the heap's
reservation, whose provenance the heap exposes when it starts (`Heap::start`), and objects that
enter magazines through the slow paths come from that reservation. Words that are not objects
(slab words, stack heads) are turned back into references only through the magazine area
(`Area::magazine`), so the pointers built for them with `without_provenance_mut` are never
dereferenced.
