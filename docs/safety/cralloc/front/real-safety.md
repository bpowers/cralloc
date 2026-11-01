# Safety ledger: `crates/cralloc/src/front/real.rs`

`Real` wraps the routines generated from Lean (`front/generated/`). Each generated function
states, in its `# Safety` section, what the Rust side must guarantee; the Lean proof covers the
rest (the instructions do what the contract says, under any interruption). The obligations
below are those sections, discharged once here for every call in the file.

**The obligations, from the generated `# Safety` sections.**

- (R) `__rseq_offset` locates the calling thread's rseq area, which, from the call until it
  returns, is registered, or reads a negative `cpu_id` and stays unregistered.
- (W) Pop and push: `word` is `SLABS + 8 * class`, the address of a word in slab 0 (the
  specification's `class` is the word's position), for a position below 512.
- (C) The `mask` argument is `8n - 16` for the class's magazines of `n = 2^k` slots,
  `2 <= k <= 16`.
- (L) `SLABS`, the thread's `rseq_cs` and `cpu_id` and `__rseq_offset` do not
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
  compare sends to the miss path. The declaration is FRONT-01 (`front/mod-safety.md`). On musl
  the word and the area are the crate's own (FRONT-01, `front/area-safety.md`): each thread's
  area reads -1 until `cpu` registers it (AREA-04), and -2 if refused or retired, both of which
  the unsigned compare sends to the miss path. A registration made during a routine breaks
  "for the whole call": a signal handler that allocates for the first time on a thread at -1,
  between a routine's arming store and its read of `cpu_id`, registers the thread, and the
  interrupted routine then runs to its commit unprotected. Nothing here rules that out; it is
  stated under "What is trusted" in `docs/design.md`, and shown by the ignored test
  `limitation_a_handler_that_registers_a_thread_inside_its_first_sequence_unprotects_it` in `front/area.rs`.
- (W): every word address comes from `Front::word(class)` for a `SmallClass`: the address of
  `SLABS`'s first slab plus `Field::hot(class).offset()` (formed as a pointer offset, then taken
  as an address), which is 8 times the class's word
  position, `4 c + 3` for class index `c`: bytes 24 to 31 of chunk `c`, below 184
  (`front/slab.rs`, with compile-time assertions), so a word of slab 0 below the
  specification's 512. `SLABS` is the `Front` static, whose slabs are at offset 0 (a compile-time
  assertion), so its symbol, which the specification calls `SLABS`, is the first slab's address.
- (C): the mask is `SmallClass::mask`, the entry of the class lookup's mask table
  (`classes/lookup.rs`) at the class's word position, built from `SmallClass::magazine().mask()`;
  `MagazineSize` keeps `n` a power of two from 4 to 1,024, so `k` is 2 to 10. The Kani harness
  `every_mask_entry_is_its_classs_mask_or_zero` (`classes/verify.rs`) proves that entry equal to
  the magazine's mask for every class, and a test checks each size's mask against the one the
  routines were proved for (`crate::spec`, which the Lean emitter writes).
- (L): on glibc, `__rseq_offset` is glibc's and the rseq area is in glibc's static TLS for the
  thread; on musl, the word is the crate's own in `.rodata` and the area the crate's own in the
  thread's static TLS (FRONT-01, `front/area-safety.md`), neither inside `SLABS`, a static of its
  own. The stack is the thread's own. None of them is at the top of the address space (user
  space ends far below it on x86-64).
- (S): the slab invariant (below), which every writer of a slab keeps:
  the routines themselves (proved in Lean), the slow paths through `store_if_equal` and
  `stack_push` (DEPOT-SLOW-01, SPARE-01), and the drain. `Real` changes nothing about that argument:
  it is the same specification with the same writers. `Real` is private to the crate, and
  `SLABS` is public only so that the C routines the export macros define in another crate can
  name its address: its type, `Front`, has no public field or method, so no code outside the
  crate can write these slabs, with a twin or otherwise. The C entry points' routines, which
  also write them, are proved in Lean to the same invariant for every class byte
  (`c/routines-safety.md`).

## The slab invariant (S)

Shipping entries cite this statement (REAL above, GLOBAL-01, HEAP-SOURCE-01, AGE-01, CACHE-01).
For every CPU with a slab and every small class:

- the hot word is zero, a marker (`classes::is_marker`), or the address of slot `s`, with
  `1 <= s <= n - 1`, of a magazine of the class's size `n` that lies in committed magazine
  memory; and
- each spare head, and the link (slot 0) of each magazine on a spare stack, is zero or the slot-0
  address of a magazine of the class's size in committed magazine memory. Never a marker below
  the head: `stack_push` refuses to push onto a marker, so a link is always the head it saw,
  which was zero or a magazine.

Committed magazine memory stays mapped and writable for the life of the reservation (the `Os`
contract; nothing in the crate purges or unmaps the magazine area), and every access to a
magazine slot anywhere in the crate is atomic.

**Why (S) holds.** Outside the crate, nothing can write a slab field: `Slab::field` is
crate-private, and the only public ways in are the routines. Of those, pop and push keep (S)
by themselves (below); store-if-equal and stack push are `unsafe fn`s whose contracts require
exactly the values (S) allows, so a caller that breaks (S) has broken an unsafe contract. Writers
of slab fields are:

- the routines' commits (and in tests the twins', which follow the same specifications): pop
  stores `w - 8` only when `w & mask != 0`, so `s >= 2` and the new slot is `s - 1 >= 1`; push
  stores `w + 8` only when `(w + 8) & mask != 0`, so `s <= n - 2` and the new slot is at most
  `n - 1`. `classes::verify` proves both tests for every size, address and marker. The stack
  routines store a magazine address the slow path holds (below) or the link they read, which
  (S) covers;
- store-if-equal, whose contract requires `new` to keep (S). The crate's only callers
  (DEPOT-SLOW-01) pass `Magazine::word(level)` with `level <= capacity`, so slot `level + 1` in
  `1..=n - 1` of a `Magazine` handle;
- the drain, which stores its marker, zero, or (in back-out) a value it exchanged out of the same
  field, with markers mapped to zero.

Spare-stack links (slot 0 of a magazine on a spare stack) are written only by the stack push
that put the magazine there, and by whoever holds the magazine's handle: the depot stacks write
slot 0 through `Magazine::link`, and `Magazine::set_object` refuses slot 0 and the last slot.
Magazines, the magazine area and the depot stacks are crate-private, so outside the crate no
handle can be had at all, and no stack can be given a magazine of the wrong size or another
heap's. Inside the crate a handle is made only by the thread that took the magazine off a stack,
out of a slab word, or out of the area, with the size of the stack or class it came from, and
that thread then owns it (DEPOT-SLOW-01, SPARE-01, the drain's step 4).

The slow paths that write a slab array all hold the one depot and space that array is used
with (`Cache::new`'s contract, CACHE-01 in `depot/mod-safety.md`), so every magazine a slab
word or link names is in that space's area, and admitted by that depot's committed mark.

A `Magazine` handle exists only for an offset `magazine::admits` accepted: past the generation
page, aligned to its size and inside the committed bytes (`depot::verify::
an_admitted_magazine_lies_inside_the_committed_area`). The magazine area itself is aligned to
the region size (`Plan::magazines_offset`), which `Geometry::try_new` requires to be at least
the largest magazine at every region size the geometry allows, so an aligned offset is an
aligned address. So every magazine address the slow paths give a routine is in committed memory,
and so is every value derived from one.

## REAL-01: the inlined routines (`slab_pop`, `slab_push`, `current_cpu`)

**Proof.** `pop` and `push` pass `Front::word(class)` and `SmallClass::mask` for a
`SmallClass`, under (W) and (C); (R), (L) and (S) hold as above. `current_cpu` only reads
`__rseq_offset` and `cpu_id`. On musl, `cpu` calls `area::register` when it reads -1, between
routines, which keeps (R) for the routines that follow; what it cannot keep is (R) for a routine
that a signal handler's registration interrupts, as (R) above says.

## REAL-02: the out-of-line routines

`slab_cas` (`store_if_equal`), `slab_stack_push` and `slab_stack_pop`.

**Proof.** (R), (L) and (S) as above. `store_if_equal` and `stack_push` are `unsafe fn`s of
`Routines` whose callers promise the new word keeps (S) and that they hold the magazine
(DEPOT-SLOW-01, SPARE-01); `Magazine::base` is non-zero and aligned to the magazine's size, at least
32. The offsets come from `Field`, always a slot of the slab.

## REAL-03: `unsafe impl Routines for Real`

**Obligations** (the trait's `# Safety`). Each method behaves as its specification, as one step
that either commits or changes nothing, on the slab of the CPU the thread was on throughout.

**Proof.** Each method is one generated routine whose Lean contract is that specification
(`lean/Cralloc/Spec/Slab.lean`): pop and push (inline), `slab_cas`, `slab_stack_push`,
`slab_stack_pop`, and `current_cpu` for `cpu`, which is not a sequence and promises nothing
about staying current. The raw results are decoded by `StoreIfEqual::from_raw`,
`StackPush::from_raw` and `StackPop::from_raw`, which follow the contracts' return values
(`NO_CPU` is 2, never a slab word). `slabs()` returns `SLABS`'s slabs, the array the slow
path's routines address by symbol and pop and push through `Front::word`, so the trait's view
and the assembly's are the same memory.

**Provenance.** The routines take and return addresses as integers. `push` exposes the object's
provenance; `pop` takes the exposed provenance back. Every object address lies in the heap's
reservation, whose provenance the heap exposes when it starts (`Heap::start`), and objects that
enter magazines through the slow paths come from that reservation. Words that are not objects
(slab words, stack heads) are turned back into references only through the magazine area
(`Area::magazine`), so the pointers built for them with `without_provenance_mut` are never
dereferenced.

Reviewed: 3b5a3f5
