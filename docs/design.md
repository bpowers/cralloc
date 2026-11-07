# cralloc design

## Summary

cralloc is a memory allocator for multithreaded Rust programs on modern Linux. It serves
Rust's `GlobalAlloc` and `Allocator` traits and the C `malloc` family from one heap, so it
works as a crate dependency or as an `LD_PRELOAD` library.

Three ideas define it.

1. **Measure the floor, then prove the code that reaches it.** The instructions that run on
   every allocation and free are written in assembly. Each sequence is the fastest of several
   candidates measured on real hardware. It is generated from Lean and proved correct, even
   when the OS interrupts it at any instruction.
2. **Per-CPU caches with no atomics and no locks.** The common path pops or pushes a pointer
   in a cache that belongs to the CPU the thread is running on, inside a Linux restartable
   sequence, and commits with a single store. The slow paths are lock-free: every change to
   shared state is a single instruction.
3. **Metadata never lives in freed memory.** A small object's size class comes from its
   `Layout` in Rust and from its address in C; a large object's always comes from its
   address. Free memory is tracked in bitmaps stored elsewhere. Freeing a small object
   writes only to the caller's own cache, so it touches neither the object nor shared
   metadata, and it costs the same on any thread.

The allocator has three tiers. Each is reached only when the one above runs dry or fills up,
and each handles objects in larger batches than the one above.

```
alloc / free
     |
     v
front   per-CPU magazines: arrays of free pointers, one per size class
        cost: a few instructions, one store to commit, no atomics
     |  magazine empty or full
     v
middle  depot: stacks of whole magazines, one set per cache domain
        cost: two atomic instructions per magazine exchanged
     |  depot above or below its target
     v
back    class regions and their free bitmaps; memory from and to the OS
        cost: about one atomic instruction per 64 objects
```

Measurements and source references are in `docs/research/`.

## Background

### What Rust gives an allocator

Rust passes the `Layout` (size and alignment) to `dealloc`, and both the old and new sizes
to `realloc`. C passes only a pointer. With the layout in hand the allocator can compute the
size class on free without looking anything up, and when the layout is a compile-time
constant, as it is for `Box<T>` and most collection nodes, so is the size class.

Rust reaches the global allocator through four symbols (`__rust_alloc` and friends) defined
in the crate that declares `#[global_allocator]`. Code in that crate gets the allocator's
fast path inlined. Code in other crates and in the standard library calls the symbols
across a crate boundary, and without link-time optimization (LTO) also calls an empty
marker function on every allocation, which costs more than the fast path itself. A library
cannot turn LTO on for its users; it can only recommend it.

The `Allocator` trait, stable from Rust 1.100, returns the usable length of each block and
lets `deallocate` be called with any size from the requested length to the returned one.
The size-class function must therefore give the same class for every size in that interval.

### What existing allocators do

- **mimalloc** gives each thread its own pages and keeps free objects in linked lists
  threaded through the objects themselves. Its single-threaded fast path is excellent, and
  all maintenance hangs off one fast-path branch, so work is paced by allocation with no
  background thread. A free from another thread costs an atomic instruction and a cache line
  shared with the owning thread, per object.
- **tcmalloc** caches pointers in per-CPU arrays under restartable sequences and moves them
  to a shared tier in batches. A free never touches the object and looks the same on any
  thread. Its per-CPU mode registers its own restartable-sequence area, which glibc 2.35 and
  later already own, and it caches a pointer in bytes it expects the OS to overwrite on every
  reschedule. Current Linux overwrites them only when the value changes and kills a process
  that writes them itself.
- **jemalloc** uses per-thread caches of pointer arrays, sized deallocation, and batched
  flushes to shared bins.

Work-stealing runtimes free on a different thread from the one that allocated as a matter of
course. In the Rust services that motivated this project, jemalloc beat a mimalloc-shaped
allocator, and the cross-thread free path is the likely cause. Our own measurements
(`docs/research/roofline.md`, "The floor and where cralloc stands";
`docs/research/workloads.md`, "The kernel suite") agree: when objects cross threads mimalloc
takes 1.5 to 2.7 times as long as jemalloc in all but the shortest one-way bursts, and both
are several times slower than the cheapest code that can do the same job. The same baselines
show where each incumbent is strong: mimalloc on building and dropping large structures,
because it faults in huge pages; jemalloc on memory use and on churn; per-CPU tcmalloc on
large objects that cross threads.

### What Linux provides

A **restartable sequence** (rseq) is a short run of instructions that the OS promises to
restart from a known point if the thread is preempted, migrated or sent a signal before the
run finishes. glibc registers an rseq area for every thread and publishes its location
(`__rseq_offset`, `__rseq_size`).

The OS also writes the number of the CPU the thread is running on into that area, and
changes it only at an event that also restarts the thread's sequence. A thread with no
registration reads a negative number there.

Together these give mutual exclusion for free. If a sequence reads its CPU number, works on
data that belongs to that CPU, and makes its only visible change with its last instruction,
then no other thread running the same kind of sequence can observe or disturb it: another
thread can run on that CPU only after this one has been stopped, and a stopped sequence is
restarted, not resumed. `docs/research/linux-rseq-contract.md` checks these claims against
the kernel and glibc sources.

The area also carries a concurrency ID, a compact numbering of a process's running threads
that would give each thread its own cache while threads are fewer than CPUs. cralloc
indexes by CPU number instead, for three reasons. It is in every kernel that has rseq. A
thread without a registration reads zero as its concurrency ID, which is also a valid one,
so telling the two apart would cost extra instructions on every call. And current kernels
can briefly show a thread a garbage concurrency ID after a failed `execve`.

### What the hardware says

We measured candidate fast paths and the primitives around them on the reference machine
(AMD Zen 5, Linux 7.2). The full study is `docs/research/roofline.md` and
`docs/research/hardware.md`. The numbers that shaped the design:

| Operation | Cost |
|---|---|
| allocate then free one object, inlined, thread-local cache | 2.0 to 2.4 cycles |
| allocate then free, inlined, per-CPU cache under rseq | 4.2 to 5.3 cycles |
| allocate then free through two out-of-line calls, either kind of cache | 8.0 cycles: a call and return takes 4 whatever the body |
| allocate then free with mimalloc v3, and with glibc, through `GlobalAlloc` | 10 to 17, and 11 to 26 cycles |
| delay from a store to a load of the same 32- or 64-bit location through a base register | 0 cycles |
| the same delay through an absolute address (what `thread_local!` compiles to), or with a zero-extending 8- or 16-bit load | 5 to 7 cycles |
| one uncontended atomic read-modify-write | 20 cycles |
| a cache line handed from one core to another | 21 ns within an L3 domain, 79 ns across |
| first touch of a 4 KiB page; the same amount of memory inside a 2 MiB huge page | 1,750 to about 2,850 cycles (310 to about 505 ns at 5.64 GHz), by how far the page falls out of cache; 117 to 214 ns |

Four conclusions follow.

- The fast path is nearly free. One page fault costs as much as several hundred allocate and
  free pairs. The design effort belongs in not faulting, not moving cache lines between
  cores, and not executing an atomic instruction per object.
- A per-CPU cache costs two to three cycles more than a thread-local one when inlined and
  nothing when called. That buys caches bounded by the CPU count, no per-thread state to
  create or tear down, and frees that are local on every thread.
- Inlining matters more than any other fast-path choice, and it needs LTO.
- The last cycle or two depend on instruction selection and layout that a compiler does not
  control. Narrow loads and absolute addressing lose zero-delay forwarding. The processor
  delivers a straight-line run of up to 12 micro-ops at full speed and about half speed
  beyond that, and a routine that leaves only its last few instructions in a second 64-byte
  line pays a cycle. Assembly lets us choose the instructions and count the micro-ops, and
  for out-of-line code fix the size and alignment too.

## Goals

The aim is to dominate the incumbents: faster than each of them, with the same memory or
less.

1. **Speed.** Each fast path within a cycle of the cheapest code that could serve the same
   workload. Faster than mimalloc, tcmalloc and jemalloc on multithreaded Rust workloads,
   especially those that free on a different thread from the one that allocated, and on par
   with mimalloc on single-threaded ones.
2. **Memory.** No more resident memory than the leanest incumbent on the same workload,
   which our baselines show is usually jemalloc. Low fragmentation from segregated size classes and from preferring
   partly used memory over fresh memory. Memory the program has stopped using goes back to
   the OS within seconds, and the memory parked in caches is bounded, by fixed amounts for
   each CPU and each class (see "Sizing the caches"). Memory is judged where it costs
   something: on heaps of hundreds of megabytes to gigabytes, as a proportion of the heap.
   A fixed cost of megabytes, or tens of megabytes, is spent freely when it buys speed or
   fewer system calls on such heaps, and the peak of a heap of a few megabytes is not
   something we tune.
3. **One heap, three interfaces.** `GlobalAlloc`, `Allocator` and the C ABI share one heap,
   and C code can free what Rust allocated.
4. **Verified.** The assembly is proved against a machine model. Size-class and layout
   arithmetic is machine-checked for all inputs. Every `unsafe` block has a machine-checked
   proof or a reviewed written one.
5. **Robust by construction.** No locks, so `fork` cannot deadlock the child and a signal
   handler may allocate. No per-thread state, so thread exit needs no code. An all-zero heap
   is a valid heap, so the fast path has no initialization check.
6. **Pure Rust plus proved assembly.** No C. The allocator is `no_std` and never allocates,
   panics, unwinds or prints while serving a request.
7. **One platform, done fully.** cralloc is developed and measured on x86-64, Linux 7.2,
   Rust 1.99, and glibc 2.43, linked dynamically or statically, or musl. The fast path needs the calling thread to have a registered
   rseq area, which Linux has offered since 4.18. A thread without one, as under valgrind,
   still works, slowly (see "Process lifecycle"). cralloc never hands a request to another
   allocator.

## Non-Goals

- **Compaction or meshing.** Fragmentation is handled by size-class segregation and reuse
  policy.
- **Speed without rseq.** The mode that runs when rseq is missing exists for correctness
  under tools such as valgrind. It reuses the path large objects take and is not tuned.
- **Other architectures and operating systems.** aarch64 comes after x86-64 is complete; its
  measurements and proofs must be redone.
- **A musl shared library.** On musl the allocator supplies its own rseq area (see "Process
  lifecycle"), which it can do in an executable. A musl shared library or preload library is
  not supported. Statically linked musl executables are tested; dynamically linked ones are
  expected to work and are not tested.
- **Hardened-allocator features.** No guard pages, quarantine or randomization. Two
  protections fall out of the design anyway: there is no metadata in freed memory to
  corrupt, and a double free is detected when the object returns to the back end.
- **NUMA awareness.**
- **Heap profiling.**
- **Allocation in a `vfork` or `CLONE_VM` child.** Such a child shares the parent's memory
  but has no rseq registration of its own, so the fast path is unprotected there.
- **More than one instance per process.** Two copies of cralloc, for example the preload
  library and a binary that links the crate, have separate heaps and must not free each
  other's pointers.

## Implementation

### Vocabulary

- **Class**: a size class. Small classes are cached in magazines; large classes are not.
- **Region**: 4 GiB of address space that holds objects of exactly one class.
- **Magazine**: a small aligned array of pointers to free objects of one class.
- **Slab**: the per-CPU block of state that names the magazine loaded for each small class.
- **Depot**: stacks of full and empty magazines, one set for each group of CPUs that share
  a last-level cache.
- **Routine**: one of the assembly sequences generated from Lean.
- **Twin**: a Rust function with the same specification as a routine, used where assembly
  cannot run.
- **Carve**: take never-used objects from the end of a region.
- **Launder**: return the objects in a magazine to their regions' bitmaps.
- **Purge**: return the physical memory behind free objects to the OS.
- **Heartbeat**: the bounded maintenance done each time a slow path runs.

### Address space

At first use cralloc reserves 16 TiB of address space, inaccessible and uncommitted. About
3940 regions of 4 GiB hold objects and the rest holds metadata and magazines. If the process cannot have
that much address space, as under an address-space limit or valgrind, the region size is
halved until the reservation fits, so the number of regions stays the same. The floor is
one huge page, 2 MiB, to a region, and the regions are as many as ever, so the smallest
reservation is about 8 GiB of address space. A process allowed less than that beside what
it maps itself cannot start the allocator, and every allocation fails. A region is
assigned to a class on demand and holds only objects of that class, at
`region_base + origin + index * class_size`. A small class's origin is a few whole pages,
different for each class, so that the first pages of the classes in use do not all share
the low page-number bits that index the TLB's sets; the pages below it are never committed,
so it costs address space and no memory. A large class's origin is zero. A 4096-byte
table records the class of each region.

A class takes a region in two steps: it writes its tag in the table, and then names the
region in the word that says where the class is carving. When two threads each take a
region for the same class, one of them loses the second step and takes its tag back, and
that region can then go to another class. So a tag read from the table is the region's for
good only if the region has been named. A caller that holds a pointer into the region
knows that it has: no pointer exists into a region that was not named. The purge and the
other walks find regions by number and know no such thing, and a walk that acted on a tag
it had read just before the tag went back would work on another class's region with the
wrong layout. So the walks never read the table. They read a second byte for each region,
which the thread that named the region writes once, after naming it. A class's region is
never given back, so that byte never changes, and a region named but not yet written
there is passed over for that moment.

Every pointer therefore has two properties that hold without per-object metadata:

- its class is `class_of[(ptr - heap_base) >> region_shift]`, one load from a table that
  fits in the L1 cache;
- its index within the region is `(ptr - region_base - origin) / class_size`.

The Rust entry points do not need even that for small objects: they compute the class from
the `Layout`. The table serves the C entry points and large objects. The scheme also removes
external fragmentation between classes, because any free slot in a region fits any request
for its class.

The price is that objects of different classes never share a page. A program that uses many
classes lightly has one partly used page per class, and each hot class needs its own TLB
entries. The huge-page policy below is built around this.

A region becomes writable in steps, each a single `mprotect` that extends the writable
prefix by about as much as the region has made writable already, from a few pages at first
to at most 64 MiB a step. Each call holds the kernel's lock on the process's address space
and on the region's mapping, and page faults in the region and every other mapping change
wait behind it, so the number of calls must grow with the logarithm of a region's use and
not with the use itself. If the OS refuses a step, the region asks for the smallest one
instead. Under strict overcommit the commit charge is the writable prefix, not the
reservation: at most about twice what a region has used.

The rest of the reservation holds metadata: each region's bitmaps at an address computed
from the region number, and an array of magazines. A small class's region keeps its
watermark and its other header words apart from its bitmaps, together with the other
regions' in the allocator's static state, so it touches none of its metadata pages until
its objects first come back to the bitmaps, and a small process pays no pages or page
tables for bitmaps it never uses. A large class reads and writes its metadata on every
allocation and free, so its header stays beside its bitmaps and sizes. Memory that holds
application objects never holds allocator state.

### Size classes and alignment

A request is first rounded up to a multiple of its alignment. Classes are every multiple of
8 bytes up to 64, then four per doubling. The class of a request is the smallest class at
least as large as the rounded size.

Alignment needs no further work. Regions are aligned to their size, a class's origin is
a multiple of the largest power of two that divides its class size, and objects sit at
multiples of the class size from there, so an object is aligned to that power of two. With four classes per doubling, the class chosen for a size that is a multiple
of `A` is itself a multiple of `A`. Every alignment up to the region size is satisfied with
no over-allocation and no interior pointers. This property, and the `Allocator` requirement
that every size from the request to the class size maps to the same class, are proved for
all inputs with Kani.

The C entry points apply C's rule: a block of 16 bytes or more is 16-byte aligned. So
`malloc(24)` returns a 32-byte block where Rust's `Layout` of size 24 and alignment 8 gets a
24-byte one.

Classes up to 32 KiB are **small**: they are cached in magazines, and Rust
computes their class from the layout. Everything above is **large**: uncached, with the
class always read from the address, because a large object may sit in a slot bigger than it
asked for (see "Large objects").

### Front end: magazines under rseq

Each CPU has a slab. Its hot part gives each small class a 32-byte chunk: the class's
8-byte word at bytes 24 to 31, and at bytes 0 to 23 words that only slow paths use. The
position is chosen against one hazard. A load that shares a page offset with a store still
in flight can be held until the store completes, and the hot word is the last thing a call
stores, so a load at its page offset early in the next call makes a loop of calls run one
at a time. Nothing a fast path loads on every call may therefore share a page offset with a
hot word. The crate places its own tables clear of them. A thread's rseq area is 32-byte
aligned and the routines touch bytes 4 to 15 of it. The copy of the C library's rseq offset
that an executable reads is clear whenever it is 16-aligned, since it then lies at bytes 0
to 7 or 16 to 23 of a chunk; the linker decides that, so a test and the build checks flag a
binary where it is not. A colder part of the slab holds counters. The slabs are a static
array with an entry for every CPU number the OS can produce (8192 on x86-64), so the address
of a slab word is the CPU number shifted and added to the address of the class's word in
slab 0, a link-time constant plus 32 times the class, with no pointer to load. Untouched
slabs cost no memory.
A routine compares the CPU number with the array length and takes the slow path if
it is out of range. That one compare keeps a thread with no rseq
registration, which reads a negative number, away from every slab, and it means no value the
OS writes there can index outside the slabs. A wrong value in range would put two threads on
one slab, so that the number is right is trusted (see "What is trusted").

A **magazine** is a block of 8-byte slots, a power of two in size and aligned to its size.
Slot 0 is a link word used by the depot, the last slot holds the fill level while the
magazine is not loaded, and the slots between hold pointers to free objects. Magazines are
numbered blocks in one array in the metadata area; they are created by advancing a counter
and are never unmapped. Their size falls as the class size rises, from 1,024 slots to 4, so
no magazine holds more than about 64 KiB of objects.

The slab word for a class is the address of the next free slot in the loaded magazine.
Because a magazine is aligned to its size, the low bits of that address are the slot
number, and the bounds checks need no other state. For a magazine of `n` slots the mask is
`8n - 16`, which covers every slot-number bit but the lowest:

- allocation takes the slow path when `word & mask` is zero, which means slot 0 or 1: no
  magazine, or an empty one;
- free takes the slow path when `(word + 8) & mask` is zero, which means slot 0 or `n - 1`:
  no magazine, or a full one.

A word of zero fails both tests. A slab that has never been written, which is all zero
bytes, therefore behaves as a slab with no magazines, and nothing needs initializing before
the first call. A slot address has its low three bits clear. A **marker** is a word whose low
19 bits are exactly 1, which leaves every slot-number bit clear, so a marker also fails both
tests (see "Idle caches").

The routines rely on one invariant, which the slow paths maintain. For every CPU and small
class, the slab word is zero, a marker, or the address of slot `s` of a magazine of that
class that nothing else refers to, with slots 1 to `s - 1` holding pointers to distinct free
objects of that class that appear nowhere else.

A routine names no class. It is given the address of its class's word in slab 0, and the
class's mask; allocation has this shape (the exact instructions are chosen by measurement and
may change):

```asm
                                      ; rdi: SLABS + CLASS*8, esi: the class's mask
    mov   r10, [rip + word]           ; the address of __rseq_offset, from a word of our own
    mov   r10, [r10]                  ; where glibc put this thread's rseq area
    lea   r11, [rip + descriptor]
    mov   fs:[r10 + 8], r11           ; arm: tell the OS which sequence we are in
start:
    mov   r8d, fs:[r10 + 4]           ; CPU number
    cmp   r8d, SLAB_COUNT             ; out of range, or no registration
    jae   slow
    shl   r8, SLAB_SHIFT
    mov   rcx, [rdi + r8]             ; next free slot of the loaded magazine
    test  ecx, esi                    ; no magazine, or empty?
    jz    slow
    mov   rax, [rcx - 8]              ; the object
    lea   rcx, [rcx - 8]
    mov   [rdi + r8], rcx             ; commit
end:
```

When the class is a compile-time constant, the caller's computation of the two inputs folds
to a `lea` and a `mov`; when it is not, to a compare, a load from a table of classes by
size, a `lea` for the word and a second load for the mask. The class's byte and its mask
come from one static, placed clear of the page offsets of every slab field. The routine
takes the mask in a register on purpose. A routine that loaded the mask itself would save
the run-time case an instruction and cost the constant case, which has the mask as an
immediate, half a cycle a pair or more; and the second load is not on the critical path,
which runs through the CPU number to the slab word (`docs/research/roofline.md`, "A store
whose address arrives late holds up the next load").

An executable can read `__rseq_offset` with one load instead of two, because the linker
gives it its own copy of the variable; that saves a cycle per pair, and a cargo feature
selects it. A shared library must go through the indirection shown. The linker would give it
a slot in its offset table for that, wherever the slot happens to fall; each routine instead
reads the variable's address from a 16-aligned word emitted with it, which the loader fills
as it would have filled the slot, so the load never shares a page offset with a hot word.

If the thread is preempted, migrated or signalled anywhere between `start` and `end`, the OS
resumes it at an abort handler that jumps back to the arming store. Nothing before the last
instruction changes shared state, so an aborted run leaves no trace. Free is the mirror
image: it stores the pointer into the free slot, which nothing can observe yet, and commits
by storing the incremented word.

The routines follow rules that come from the measurements, and the generator refuses to
emit one that breaks them:

- state that the fast path both writes and reads is 64 bits wide and addressed through a
  base register;
- a compare and its branch are adjacent, so they fuse into one micro-op;
- the commit stores a value computed from the word the routine read. It never updates the
  word in place with a read-modify-write instruction, because a drainer may exchange the
  word between the read and the commit (see "Idle caches");
- an inlined routine is at most 13 micro-ops, one more than the processor delivers at full
  speed. Inlined allocation costs its caller one more, a fused test for a miss;
- an out-of-line routine starts on a 64-byte boundary, and its hot path fits in 24
  micro-ops, which is what the processor delivers in the four cycles a call and return take
  regardless. Its hot path is at most 64 bytes, or between 76 and 128: crossing into a
  second line is free unless only the last few instructions do, and a third line costs a
  cycle;
- the slow path is out of line.

The pop and the push are used in two settings. For Rust they are `asm!` blocks in
`#[inline]` functions that take the word's address and the mask in registers. They become
part of `__rust_alloc` in the user's binary and, with LTO, of its callers, whether their
layouts are constant or not. For C they are the last steps of the entry routines for
`malloc`, `calloc`, `free` and the sized free, which are out of line and exported as the C
symbols themselves. An entry routine first turns its argument into the word's address: a
size through a byte table of classes, a pointer through the region table. It reads the mask
from a table placed after the slabs at a fixed distance from the word, one entry per
possible class byte. That table is zero until the first slow path writes it, and a zero mask
fails both tests, so it needs no initialization check, and every byte that is not a small
class sends a free by address to the slow path.

### Slow path: exchanging magazines

When the fast path fails, the slow path replaces the loaded magazine instead of refilling
it. For an allocation it:

1. obtains a full magazine: from the CPU's spares, else from the depot, else by filling an
   empty one from the back end;
2. installs it with a routine that stores the new word only if the slab word is still what
   the slow path read there: zero, an empty magazine, or a marker left by another process
   generation. The routine returns the old word;
3. retires the old magazine, now private to this thread, to the CPU's spares or the depot,
   and retries the allocation.

If the install fails because another thread got there first, or because this thread moved
to a CPU that already has a magazine, the slow path puts its magazine back and retries. If
it fails because the word is a live marker, or because the thread has no valid CPU number,
the slow path serves the request from the magazine it is holding and sends that magazine to
the depot, so it never waits. The free side is symmetric with an empty magazine.

Each CPU keeps spare full and empty magazines per class, in stacks whose heads live in the
cold part of the slab and are updated by single-store routines. A thread that oscillates
around a magazine boundary swaps with a spare and touches no shared state. The swap is the
commonest slow path, so it carries nothing it does not need: its handles are single words
that travel in registers, and it runs no maintenance of its own. How many spares a class
keeps on a CPU is a fixed limit, which for full magazines is none for most classes, and
beyond it whatever stock the class has earned there ("Sizing the caches" has both). A
magazine the spares have no room for goes to the depot, and a trip to the depot moves one
magazine.

A swap installs a magazine at one of its ends: full for an allocation, empty for a free.
Work that swings both ways from where it started, a few allocations of a class and then
more frees than that, then crosses the far end of the new magazine, and so does every
repetition after it: two swaps each time, for as long as the work is steady. A server
whose requests do this ran several percent slower in the processes where its first
magazines happened to land that way (`docs/research/magazines.md`, "The magazine-end lock").
So two reversals in a row, a swap that reverses a swap of that class on that CPU which
itself reversed the one before, are taken as evidence of such a lock, and the slow path then
leaves the loaded magazine half full: it takes the loaded magazine out of the slab and moves
half of one magazine's objects into the other while it holds both. A swing of less than half
a magazine never leaves the loaded magazine again, and servers whose requests swing this way
then take almost no slow paths at all (`docs/research/magazines.md`, "When a server's load
reaches the slow path"). A flow never reverses, and a swing longer than a magazine reverses
once at each turn, not twice, so neither pays for the move.

Half a magazine cannot absorb a swing longer than that, and a swing between half a
magazine and a whole one would be centred at every turn without ever fitting. So the rule
backs off. A centred swap has failed if another swap of that class on that CPU follows it
within an interval, whichever way: a reversal is a swing the half did not hold, and a
second swap the same way is the long leg of one. Each failure in a row doubles the number
of the swaps after it that stay plain, up to a bound, and only an interval without a swap
clears the count.
A swing the rule cannot help then pays for an occasional retry and nothing else, and one
that changes into a swing it can help is centred at the next retry.

Every step is one commit. Between steps all shared state is valid and whatever the thread
holds is private to it. This is what makes the slow path safe to interrupt with a signal
handler that allocates, and safe to abandon at any instruction in a forked child.

### Depot

The depot holds, per class, a stack of non-empty magazines for each cache domain (the CPUs
that share a last-level cache) and one more shared by all of them, and per magazine size
the same for empty magazines. A slow path that retires a magazine pushes it on its domain's
stack while that holds less than a cap, else on the shared stack. One that needs a magazine
pops its domain's stack, else the shared one, else fills or makes one. The domains are read
once from sysfs, on the process's first
slow path and never later, so that a program which has since entered a sandbox is not caught
opening a file; where they cannot be read there is one domain. A stack head is one 64-bit
word: a 32-bit magazine number and a 32-bit version
counter, updated by compare-and-swap. Magazine memory is never unmapped, so reading a stale
link is harmless, and the counter means a stale head is accepted only if exactly 2^32 pushes
and pops intervene during one attempt.

Exchanging through the depot is how objects move between CPUs. A thread that only frees
fills magazines and pushes them; a thread that only allocates pops them. An element of a
stack is one magazine, and its link also carries the stack's count up to it, so the top
magazine's link says how much the stack holds without a counter of its own. A magazine
exchanged costs two atomic instructions and no work per object, however frees and
allocations are spread across threads.

Magazines stay in the cache domain that freed them. An object's cache lines live where it
was last used, and a magazine handed to a CPU under another last-level cache brings objects
whose every first touch is a miss served from the other cache: with one set of stacks for
the whole machine, two unrelated pairs of threads on different domains traded magazines
through it and each ran at half speed.

A domain in balance keeps its magazines: they reach the shared stack only at the ends of
its range. A flow of objects from one domain to another fills the freeing domain's stack to
its cap once and then passes through the shared stack, at two atomic instructions a
magazine, so objects still cross domains without touching the bitmaps when the program
really moves them across. No slow path reads another domain's stack, so its work does not
grow with the number of domains, and a flow between two domains cannot stall behind a third
that happens to hold the class.

Each class has a target for the bytes of objects its depot stacks hold in a process that
runs in one domain: a constant, the same for every class (see "Sizing the caches"). A
domain stack's cap is a quarter of it and the shared stack's target is the rest, and each
is at least one magazine.
A domain stack cannot pass its cap, the heartbeat launders the shared stack while it is over
its target, and every stack gives up whatever it did not need during an interval (see
"Returning memory"). A class in use on N domains parks at most N caps and the shared target.

### Back end: regions and bitmaps

Each region has a free bitmap with one bit per object, and above it a tree of summary
bitmaps, 64 to 1, in which a set bit means "something below may be free". A region also has
a watermark: objects at or above it have never been allocated, and their memory has never
been touched.

- **Filling a magazine** searches the class's regions in address order. In each, it walks
  the summary tree to the lowest bitmap word with free objects and claims bits from it with
  one compare-and-swap. The objects of one word are adjacent in memory, so a refilled
  magazine is spatially dense. A fill goes on from the word it last claimed from, without
  walking down from the top again. When it meets another fill at a word (the word changed
  under it, or was emptied before it arrived), it moves on past that group of 64 words, so
  fills of one class on different CPUs part after a few collisions instead of contending
  for every word; they stay near the bottom of the heap. A fill asks for a whole
  magazine. It claims from a bounded
  number of words and then hands out what it has, so in a heap whose free objects are
  scattered one to a word it returns a partly filled magazine instead of searching on, and
  no call pays for more than that many claims. If no region of the class has a
  free object, which only a search from the top may conclude, the fill carves from the last
  region's watermark with one atomic add, and when that region is exhausted it assigns a
  new one to the class. It does not carve while it can see a free object. A free object
  can be out of sight for a moment, while the thread that emptied its bitmap word has
  cleared the word's summary bit and not yet read the word again, and a search in that
  moment may carve although the object is free.
- **Laundering a magazine** groups its pointers by bitmap word and sets each word's bits
  with one atomic OR. A magazine whose objects are scattered, each in a word of its own, has
  nothing to group: once a batch shows that, each remaining object gets one atomic bit-set
  and no grouping is built. Either way, if a bit was already set, the object was freed twice
  and the process aborts.
- **Summary bits are hints.** They may be set when nothing below is free, and they never
  stay clear when something is. A free sets the word's bits and then sets every summary bit
  above it that it finds clear. Whoever empties a word clears its summary bit, at once or
  together with the others it emptied under the same summary word, and then reads each word
  again and sets back the bit of any that is no longer empty. A fill that finds a word empty
  under a set bit leaves the bit to whoever emptied it; a search from the top clears what it
  finds empty. A free does not assume that the first free into an empty word set the
  summary bit, because that thread may have vanished in a fork before it could.

The order of preference in a fill is the fragmentation policy: reuse free objects before
carving fresh memory, and take the lowest address first. Live data packs toward the start of
each region and the high end drains. It also makes the highest slot a fill reaches a measure
of how much of the region the program needed, which the purge relies on (see "Returning
memory and huge pages").

A filled magazine also hands its objects out lowest address first, so that the order within
a magazine is the order across magazines. A program that allocates a run of objects and then
visits them in the order it made them, which is what building and walking most linked
structures does, then reads memory in one direction, and hardware prefetchers follow it. If
each magazine handed its objects out highest first, the direction would reverse at every
magazine, and the prefetchers would lose the stream each time.

One fill is the exception. Objects of one size allocated one after another are often
siblings: one for each worker thread, made in a loop and then written by different threads
for the life of the process. Back to back in a class smaller than a cache line they share
lines, and every write by one worker stalls its neighbour. A runtime's per-worker queue
headers sat that way, and a program that did nothing but spawn tasks lost a seventh of its
time to them, where an allocator with other size classes kept them apart by luck
(`docs/research/program-effects.md`, "False sharing"). So in classes smaller than a line,
the fill that opens a run, one whose region shows no claim earlier in the interval, hands
out its first objects a stride of slots apart, far enough that no two share a line, and then
the ones between. No slot is skipped. Only the order changes, for a bounded part of one
magazine, and a fill that continues a run is dense, so a program building a structure has
its stream back after the first magazine. The same order on every fill cost such programs up
to half again of their time.

### Heartbeat

Slow paths do the allocator's maintenance, a bounded amount at a time. It is paced by what
moves, not by calls: a slow path adds the bytes it moved between a magazine and anything beyond
it to a count in its CPU's slab, and runs the maintenance, a heartbeat, when the count
passes a threshold. A swap with a spare then does no maintenance of its own, and
the rate of maintenance does not change when a class's magazines, its trips to the depot or
its fills change size. A large allocation or free counts as a slow path, and beats on one
call in sixteen: a program that moves large blocks makes such calls far more often than the
maintenance needs to run. The work
is done in units small enough that no single call pays much. This is mimalloc's pacing
applied to a per-CPU design.

A heartbeat does one step of purging, one look at a class of its own slab, one look for an
idle cache, and laundering within a budget of one of the largest magazines. The rules that
launder share that budget. A rule that finds work and no room waits for a later heartbeat.
The rules that only a CPU's own slow paths run for it, the age limit and the sweep, would
wait for ever on a CPU whose every exchange spends the budget, so one that was turned away
goes first at that CPU's next slow path, which runs a heartbeat whatever the count. One launder is not paced and not budgeted: a
magazine that a call's own exchange pushed past a shared depot stack's target is laundered
on that call, so that the stack cannot gain on what arrives. A call therefore launders at
most one of the largest magazines, or two when it also carries a rule that was turned away,
which happens at most once an interval for each class and rule.

How often a slow path runs depends on how hard the process allocates, so anything that
should happen at a rate in time reads the CPU's cycle counter, which costs no system call,
and acts once the counter has advanced by a fixed count, 2^32 ticks, since it last did.
"An interval" below means that. The allocator does not learn the counter's rate, which
would take a system call, so an interval's length follows it: about a second where the
counter runs near 4 GHz, about two where it runs at 2 GHz, and between 0.7 and 4.3 seconds
on any plausible machine. Every time given below stretches the same way: the hold on free
large slots is an eighth of an interval, and "within seconds" for idle memory means within
a few intervals.

Intervals are counted, not computed from the clock. The count steps by one when a heartbeat
finds that an interval has passed since the last step, however long ago that was, so a
process that was silent for a minute has aged everything it holds by one interval, not
sixty. The evidence the rules act on is that something went unused while the program was
running, and a silence is not that. Counted by the clock alone, the first requests after a
silence find every cache marked idle and pay to tear down what they are about to use: a
server driven in bursts spent the first milliseconds of each burst purging single pages
and draining other CPUs' slabs (`docs/research/servers.md`, "The first requests after a
silence"). Memory that should go back during a silence needs something that runs during it,
which is what the rest of this section is about.

Nothing runs on the fast path, so a thread whose allocations and frees are all served by
its loaded magazines never runs a heartbeat. Other threads cover for it: any slow path
maintains the shared structures, and the idle check drains a slab that has had no slow path
for an interval, which sends that CPU to its slow path. Only a process in which every thread
stays inside its magazines has no heartbeat at all. It keeps what it held when it went
quiet: the free memory not yet purged, its parked objects, and the pages those pin. That
lasts until a slow path runs or the program calls `collect`.

Going quiet is where steady work on one thread ends up, not a corner case. A swing of
allocations and frees smaller than a magazine settles, after a few swaps, into a loaded
magazine that holds exactly one swing, and never leaves it. A program that frees a burst
and then works lightly on the thread that freed it kept the whole burst in more than half
of its runs (`docs/research/memory.md`, "After a peak, and in a silence"). Programs whose
threads free each other's objects, or grow buffers, enter slow paths constantly and are not
affected. Nothing on the freeing path can stand in for the clock here: a free cannot tell
memory that is idle from memory about to be reused, and purging on a level makes the
programs that reuse it fault it straight back.

So there is a background thread, and by default it starts itself, once two things are
true together: the process holds more free memory that is still resident than the floor
(see "Sizing the caches"), and the program has created a thread of its own. Objects parked
in magazines and the depot are not in that count. A
small program's thread never runs, because the program holds nothing worth returning, and
a program with one thread never gets one, because it may depend on staying that way. Once
quiet, that program keeps its free memory until it calls `maintain` or `collect`, or starts
the thread itself. glibc's allocator keeps the same memory in the same program, and
matching it is enough: we accept this as a limit and build no mechanism for it. The
evidence of a second thread is the C library's own record that one was created; a library
that keeps no such record leaves a free made by a thread other than the first as the
evidence. Which thread happens to free the memory must not matter where the library can
say. The library's record survives a fork, and a fork child begins with one thread
whatever its parent had, so in a fork child the evidence is again a free by a thread other
than its first: a child is the process most likely to depend on having one thread, and
keeping some memory there is the cheaper mistake. The condition is a level and not an event. It is looked at when free memory rises
over the floor, and again once an interval for as long as it stays there, so a moment at
which the thread could not be created only delays it. The thread buys nothing on
the fast path, which stays as it is. It wakes every half second, which is more often than
the shortest interval, and calls `maintain`, which runs one round an interval and does for
the whole heap what the heartbeats of an interval would have done: the idle check on every
slab, each depot stack's shedding, and the clocked purge, in bounded steps. A slab it
drains has its magazines laundered, not parked in the depot: a CPU that is busy only on
fast paths would otherwise take them straight back, and their scattered objects would go
on pinning pages.

The thread sleeps while there is nothing for it to do. Its rules act on what one interval
or several left unused, so a round that finds nothing does not show that the next will:
a slab drained in one interval is shed in the next, what was shed is purged by a pass
after that, and scattered pages wait for the pass that takes runs of any length.
So the thread parks when its rounds have found no work for as many intervals in a row as
the longest of those waits, and no slow path has run anywhere in that time. It then waits
on a word with no timeout, and a process that has gone silent is not woken by its
allocator at all. How much memory the process still holds is not part of the rule. A heap
can hold more than the floor in free slots on pages that also hold live objects, which no
round can return, and a thread that stayed awake for those would wake for ever and return
nothing. By then
its rounds have drained every slab, so the first allocation or free after the silence
takes a slow path, and a slow path that finds the word set clears it and wakes the thread.
That costs each slow path one load of a word that is otherwise not written, and the fast
path nothing. A program that is working, even one that never leaves its magazines, keeps
the thread awake, because each round's drain sends its CPUs to their slow paths. Where
slabs cannot be drained (see "Process lifecycle") nothing would send a resuming program to
a slow path, so there the thread keeps to its timed sleep and never parks.

While the thread runs, returning memory is its work and not a slow path's. A request that
returns memory makes every request queued behind it on that worker wait, and a server
whose tables grew by doubling had its slowest doublings set by the purge steps that
happened to fall on them, not by the copy (`docs/research/large-blocks.md`, "A doubling's
tail is the purge that falls on it"). So a slow path that finds such work due, a hold that
has matured, growth over the cap to be paid for, an interval's pass, does none of it. It
records what is due and, if the thread is asleep, wakes it. The thread takes the same
bounded steps under the same rules until nothing is due. Before it sleeps it publishes when
its next look at the holds falls due, in counted time, and the heartbeat whose reading of
the clock reaches that wakes it. Counted time moves only when a slow path reads the clock,
so a timer of the thread's own could not follow it, and in a silence would wake the thread
for holds that cannot mature. What goes back, and when to within milliseconds, is unchanged.
The thread starts when free memory first passes the floor, and the clocked rules return
nothing below the floor, so in a program that has the thread a request's slow path takes
none of their steps. What a request still returns is what its own call decides, each of
it listed under "Returning memory and huge pages": the trim at the claim of a large slot,
the tail of a block that shrinks in place, the pieces behind a block that moves, and a
freed slot that was under huge-page advice or left unwritten. Slow paths keep the clocked
work where there is no thread: a program with one
thread, one that turned the thread off, one in which it could not be created. That is why
every step stays bounded (see "Returning memory and huge pages"). And the bound on
retained memory must not rest on the scheduler. A thread that is starved or throttled does
not run, so when growth has gone unpaid past a limit the carves and heartbeats that find it
pay a step each themselves, and a pass that nobody has stepped for an interval is the slow
paths' again. The thread does not give up the CPU between its steps. Yielding after each
one handed a busy worker a whole time slice every time, and the thread fell far enough
behind on holds and payments to cost a query engine its peak in half its runs, while no
server's tail moved (`docs/research/memory.md`, "Returning memory off request paths").

It is an ordinary thread of the process, created through the C library by the layer that
links one, not by the allocator with a bare system call. A bare thread could be created
anywhere a slow path runs, but the C library would not know it, and what the library does
to every thread, a change of credentials above all, would pass it by. The price is that
creating a thread through the library is not safe everywhere a slow path runs: not in a
signal handler, and not inside the library's own locked sections, which is where its
allocations for a new thread or a newly loaded object come from. So the two layers choose
their moment differently. In a Rust program the thread is created on a heartbeat of a
call that came through the Rust interface, which the C library does not use, and only if
the calling thread has no signal blocked and is not on a signal stack, the marks of a
handler; otherwise the next look at the level tries again. A Rust program that keeps a
signal blocked in every thread never passes that test, and starts the thread itself. Two
cases the test cannot see are limits we accept. A handler installed to run with its own
signal unblocked (`SA_NODEFER`) and on no stack of its own carries neither mark, so if the
one slow path that creates the thread, once in a process, runs inside such a handler, the
thread is created from a signal handler, which can deadlock against whatever the handler
interrupted inside the C library. And a slow path inside a callback of the C
library's `dl_iterate_phdr`, as when a backtrace is being symbolized, runs under a loader
lock, and a thread created there can deadlock against another thread inside `dlopen` or
`dlclose`. It takes the one slow path that creates the thread, once in a process, to fall
inside such a callback at that moment, and the lock is private to the library, so no slow
path can test for it. The
preload library, whose caller may always
be the C library, creates the thread when the program creates its own first thread, a
moment that is safe by construction, and leaves it dormant until the heap is over the
floor: the thread looks at the level before it first sleeps, and the free that takes the
heap over the floor wakes it. To see that moment the library exports `pthread_create`
itself and passes every call on to the C library's. Threads the C library creates for its
own purposes do not come through it, so a program whose only other threads are those gets
no thread. A program can turn the thread off, with a cargo feature or
an environment variable, and can start it itself whatever the conditions say. The
variable is read once, at the process's first slow path, from the environment the process
was started with, by the allocator's own system calls. The C library's `getenv` is not
safe against a `setenv` in another thread, and a slow path cannot know what the program's
other threads are doing; a read that waited for the first use could also come after the
program had entered a sandbox that forbids opening files. A process that gained
privileges when it was executed cannot read its own environment this way and ignores the
variable, as the C library's `secure_getenv` would. The thread takes the C library's
default stack. The library puts the thread's static thread-local data on that stack, and
glibc grants a smaller request that leaves almost nothing beside the data, on which the
thread would crash. Sizing a request to fit needs the loader's list of modules, which is
read under a lock that a fork does not release in the child and that the program's own
callbacks run under. The default is sized by the library against that data, and a smaller
stack would save only address space (`docs/research/portability.md`, "The background
thread's stack"). A program that gives all its threads a stack of a few kilobytes gives this
one the same. The thread names itself with a system call of the allocator's own, so that
nothing opens a file when it starts. If the thread cannot be created at all the allocator
carries on without it. A process under a system-call filter that kills on thread creation,
which no allocator can detect, must turn it off. The child of a fork does not inherit the
thread and starts its own on the same conditions, its own first thread included. The word
that says whether the thread exists also names the generation it belongs to (see "Process
lifecycle"), so that claiming the right to create it is one atomic instruction, a fork child
reads its parent's word as stale, and a child that shares its parent's memory is not taken
for a new process.

A program with a timer of its own can call `maintain` from it instead, and a process that
calls `maintain` does not get the thread by default. In an async runtime that is a task
that wakes every half second, as the thread does, and we ship one for tokio, in a crate of
its own because the allocator has no dependencies. Both are offered because neither fits
every program: a server without an async runtime has no timer to borrow, and one that must
stay single-threaded cannot take the thread. The task costs two wake-ups a second,
it runs while a server is silent, which is when a burst's memory should go back, and it
takes that work off the first requests of the next burst. A round of maintenance is
taken in bounded steps, and the task yields to the runtime between them, so a runtime
with one thread is not held up by it. `maintain` limits its own rate,
returning at once unless an interval has passed since it last ran, so it is harmless to
call it more often, as from a hook that runs when a worker is about to park. Such a hook
does not replace the timer: a worker parks once at the start of a silence, and nothing
calls the hook again until the silence is over.

Objects mostly circulate through magazines and the depot without touching the bitmaps. They
reach the bitmaps when a class's shared depot stack is over its target, and through the rules under
"Returning memory" that keep parked objects from pinning the heap.

### Sizing the caches

Every buffer in the allocator stands between a cheap tier and a dearer one. A loaded
magazine stands between the fast path and a swap with a spare; a CPU's spares between that
swap and a trip to the depot; the depot between two atomic instructions and the bitmaps;
free memory that is still resident between the bitmaps and the kernel. Each has a bound,
and the bounds are constants: one for each class, each CPU or each cache domain. They do
not scale with the heap, and but for one rule, stock, they do not adapt to the program. So
what a process parks in its caches depends on how many CPUs it runs on and how many
classes it uses, and not on how much it has live. The fast path pays nothing for any of
it: a magazine's size is fixed per class, and everything else is read and written by slow
paths. A design in which every one of these sizes follows the program was studied and not
built; `docs/research/magazines.md`, "Sizing on evidence", has it, and
`docs/research/prior-art.md`, "How others size their buffers", the prior art.

The bounds:

- **A magazine** holds at most 64 KiB of objects, in 4 to 1,024 slots. That one size is
  both how far a thread can swing between allocating and freeing without leaving the fast
  path, and how much moves in one exchange with the spares or the depot. A fill from the
  bitmaps asks for a whole magazine.
- **A CPU's spares.** A megabyte of objects for each CPU is split evenly among the small
  classes, and a class keeps as many full spares as fit in its share, four at most. A
  full magazine of most classes is bigger than a share, so only the smallest classes keep
  a full spare this way, and the rest keep one only as stock. An empty spare holds no
  objects, and every class keeps up to four.
- **Stock** lets the classes a CPU uses take the shares of those it does not. It is the
  one bound that follows the program, and the next paragraph describes it.
- **The depot.** A class's stack in a cache domain takes magazines while it holds less
  than 32 KiB of objects, and the shared stack is laundered down to 96 KiB: 128 KiB a
  class in a process on one domain, and at least one magazine in each stack whatever the
  class (see "Depot").
- **Free memory that is still resident has a floor**, an allowance of 32 MiB. Under it
  the clocked rules return nothing, and the background thread has no reason to start. The
  floor is generous on purpose: a heap under it is small enough that what it keeps does
  not matter (see "Goals"). Above it the rules under "Returning memory and huge pages"
  apply, and their cap on what a growing heap may retain, 64 MiB, is a constant too.
- **The bounds on work, and the interval, are constants.** How much one call may launder,
  purge or fill, and how long an interval is, bound latency.

Stock is spare magazines a class keeps on a CPU beyond its fixed limit. A thread that
allocates many objects and then frees them all, as a request handler does, moves more
magazines in a burst than the fixed spares hold, and each one past them goes to the depot
when it is freed and comes back when it is wanted. Against a depot stack nobody else is
using that costs little. Against one that other CPUs are using it costs a cache line
handed between cores and compare-and-swaps that fail. So a CPU keeps what it has shown
that it sends away and takes back, when the sending is dear. A slow path that sends a
magazine to the depot because its spare stack is full marks the class on that CPU. When a
slow path of that CPU next takes a magazine of that kind and class from the depot, the
round trip is complete, and if a compare-and-swap failed on either trip the class may keep
one more spare of that kind there. The mark waits for its match however long that takes.
A CPU whose traffic in a class goes one way, freeing what another allocates, never takes
back what it sent and earns nothing. Nor does a thread whose exchanges meet no
contention, however far it swings. All the stock of full magazines on a CPU, over every
class, stays within what the megabyte leaves after the fixed spares; stock of empty
magazines has the same room, each counted as if it were full, and a class keeps no more
empty stock than it has earned full. When the room is spent the next magazine goes to
the depot, and nothing is evicted. The words that record all this are hints, written with
plain stores by the slow paths of the CPU a thread is on: a lost one moves a magazine to
the depot or keeps it one exchange longer.

What a buffer holds shrinks on the clock, and "Returning memory and huge pages" has the
rules. Stock an interval did not use comes off the class's credit, and the magazines
above the new limit go back one a heartbeat. A magazine does not stay loaded for more
than an interval, full spares are swept once an interval, a depot stack sheds the depth
an interval did not need, and the slab of a CPU with no slow path for an interval is
drained. One rule acts on a level instead: a shared depot stack over its target is
laundered as magazines arrive.

Two more decisions rest on evidence and set no size. Maintenance is paced by the bytes
slow paths move, not by how many there are (see "Heartbeat"). And two swaps in a row that
each reverse the one before leave the loaded magazine half full (see "Slow path:
exchanging magazines").

### Returning memory and huge pages

A huge-page fault delivers 512 pages for about a fifth of the per-page price of small
faults, and a page returned too early is faulted straight back. Against that, memory the
program has stopped using must go back to the OS within seconds, or a process keeps its peak
for ever. Memory the program is not using sits in one of the places below, and each has a
rule that returns it (`docs/research/memory.md` and `docs/research/large-blocks.md` have the
measurements behind them).

| What holds the memory | What returns it | When |
|---|---|---|
| Free slots in the bitmaps, on pages with nothing live on them, above what fills reached in the last two intervals | The clocked purge | Once an interval while more than the floor could be returned: everything the last lap found it could return, down to the floor, in runs of adjacent pages, carrying on round the heap from where the last pass stopped |
| Free pages of a huge page above that frontier that still holds live objects, when a quarter of it or more is free | The clocked purge splits the huge page and returns them, and the advice for huge pages comes off there | With the pass that reaches it |
| Free slots of large classes, above the floor | Heartbeats, a bounded amount each; `maintain` in a silence | Once their region, or the slot itself, has gone a short hold without a free that left it resident |
| The pages of an occupied large slot past both its block and its last tenant's | The claim of the slot (see "Large objects") | At the claim, one bounded step, from the top down |
| The same, whether fills reached it or not, while more than the cap is retained and the heap grows somewhere else | The carve that grows it owes as much as it takes: it returns a bounded part, and heartbeats return the rest | At that carve and the heartbeats after it, a bounded step each |
| A slot a large block outgrew, beyond a bound on such slots that nothing has taken again | The move (see "Large objects") | Behind the copy, a piece at a time |
| The tail of a large block that shrinks in place | The resize (see "Large objects") | At once |
| Objects in a magazine that stays loaded | A slow path of the same CPU launders it | Once it has been loaded for an interval |
| Objects in a CPU's full spares | That CPU's sweep launders one magazine and sends the rest to the depot | Once an interval |
| Stock a class no longer needs on a CPU | The sweep takes the depth not needed during an interval off the class's credit and returns what lies above the new limit, full magazines to the bitmaps and empty ones to the depot; the idle drain takes it with the slab | A magazine a heartbeat, from the interval after its last use |
| Objects in a depot stack | A heartbeat launders a magazine from the shared stack. Every stack sheds what it did not need, and each periodic look visits every stack of one other domain whose own CPUs have not looked this interval, the domains in rotation over those that hold magazines, so a domain that goes quiet sheds within a few intervals | While the shared stack is over its target, and for the depth a stack did not need during an interval |
| Objects in steady circulation | The free that retires a full magazine launders it instead | One retirement in 32, and up to one in 4 while the small classes' free slots beside live objects are at least a sixteenth of what they carved |
| The slab of a CPU the process no longer runs on | The idle drain sends its magazines to the depot (see "Process lifecycle") | After an interval with no slow path there |
| The part of a huge page above a region's watermark, made resident by the fault and never allocated | The purge carves it and frees it, so that the page can go whole | When the purge reaches that page |
| Free pages of a huge page that fills still reach, or that is less than a quarter free | Nothing: the huge page is worth more whole | When the whole huge page is free, or fills stop reaching it |
| Free slots on pages that also hold live objects | Nothing: that is fragmentation, and it stays counted | Never |
| All of the above, without waiting for the clock | `collect`, and `malloc_trim` in C | When the program calls it |

Nine rules stand behind the table.

- **Purge late, and only what fills did not reach.** Purging claims the bits of every
  object that overlaps a range of pages, exactly as an allocation would. If all of them
  were free it returns the pages with `madvise(MADV_DONTNEED)`. Either way it then releases
  the bits. Nothing else can allocate the range while it is claimed, so no lock is needed.
  What to purge is decided on evidence of reuse. Fills and allocations take the lowest free
  slots of a region, so the highest slot a region's claims reached during an interval shows
  that everything below it was needed in that interval. A fill that moved past another
  fill records only what it reached before that, because what it claims beyond says
  nothing about the slots it skipped. Each region keeps that frontier for
  the interval under way and the one before, in one word that also names its interval. A
  claim writes the word only when it reaches higher or is the region's first of an
  interval, and a reader takes a word from two or more intervals back as empty, so nothing
  has to age the words and only a thread that holds a claim in a region writes its word.
  Once an interval a pass carries on round the heap from where the last one stopped and
  returns free pages above the frontiers, in bounded steps spread over the heartbeats that
  follow. Its goal is everything the last lap found it could return, less the floor. The
  frontiers already keep that to what two intervals did not reuse, and holding part of it
  back for later only keeps memory the program has shown it does not need: a pass that
  took a quarter an interval kept more at the troughs of a large heap and saved nothing
  in time (`docs/research/memory.md`, "How much a clocked pass returns"). While the
  program runs it never takes pages below a frontier, however high the count. Those are the
  pages the next fills take, and purging them only makes the program fault them back. What
  two intervals did not need goes back, which is the rule the depot stacks and the spares
  follow too. Only an interval in which some slow path ran counts toward a frontier's age.
  A silence is not evidence that the program has stopped needing what it last used: the
  background thread's rounds through a quiet half minute would otherwise age every
  frontier, return the pages its own drains had just completed, and leave the first
  requests of the next burst to fault them back
  (`docs/research/servers.md`, "The first requests after a silence"). In a silence a pass
  takes what lies above the frontiers first, and goes below them only while more than the
  floor could still be returned, each region from its top down, so that what a region keeps
  is its lowest pages, the ones its next fills take. A frontier is a hint: a wrong one costs
  a purge too many or too few, never a page in use, because the purge is a claim.
- **Purge in runs, and let short runs wait.** Returning memory costs a system call and a
  round of interrupts to the other CPUs for each call, whatever its size, so a pass
  returns adjacent free pages together, as one claim and one call, up to a bound on the
  length of a run that also bounds how long a claim is held. A single free page among
  live ones is the dearest memory to return and the likeliest to be refilled, since fills
  take the lowest free slots. So most passes take only runs of at least a few pages, and
  one pass in every few takes a run of any length: scattered pages that stay free that
  long do go back, and a program that reuses them in the meantime never paid to return
  them. A server driven in bursts spent the first milliseconds of each burst returning
  single pages it was about to refill (`docs/research/servers.md`, "The first requests after
  a silence"). Growth over the cap, and `collect`, take runs of any length.
- **A step on a request's slow path is bounded, whatever the unit.** A request that
  returns memory delays every request queued behind it on that worker, so a millisecond
  spent there once a second is the server's tail. Every step a slow path takes, of the
  clocked pass, of the hold or of a growth payment, therefore returns at most a fixed
  amount of memory. A free large slot bigger than that goes back in pieces, a step each,
  from its top down, each piece claiming and releasing the slot as a whole purge does. The
  slot counts as zero only once every page of it has gone, so a request for zeroed memory
  that takes one partly returned zeroes it, as it would any other. In a class of a huge page or more a piece is whole
  huge pages, so a purge does not split the pages it leaves behind
  (`docs/research/memory.md`, "Returning memory off request paths"). `collect` and
  `malloc_trim` are the program's own calls and are not bounded. Where the background thread
  runs, a slow path takes none of these steps and the thread takes them all (see
  "Heartbeat").
- **Free large slots do not wait for the frontier.** A large class's region serves one
  size, so a program that frees blocks of many large sizes between phases of its work
  holds them all, class by class, for the two intervals the frontier rule waits. A query
  engine held a third of its peak this way between queries
  (`docs/research/large-blocks.md`, "Free slots: a hold, and why no hold serves everyone").
  So free large slots follow a shorter clock. A large free that leaves its slot resident
  stamps its region with a fine count, which steps every couple of milliseconds that a
  heartbeat observes and at most one step for each reading of the clock, so that a silence
  ages a hold by one step, as it ages everything else by one interval. Once a region has
  gone a hold of some tens of steps without such a free, heartbeats return its free slots, a
  bounded amount for each heartbeat, while the free large memory that is still resident
  exceeds the floor, and only down to it. A region that tenants keep taking and freeing
  never goes a hold without such a free, and by the region's clock alone its free slots
  would stay for as long as the churn lasts (`docs/research/large-blocks.md`, "Each free
  slot ages on its own"). So the free stamps the slot too, in its size word, and a look at a
  region whose own stamp is young walks the region's free slots, carrying on from where the
  last look stopped, and returns those that have themselves been free for the hold. A region
  is walked at most once a hold, whoever looks, so the walk costs one pass over its free
  slots a hold, and a slot goes back within a few holds of its free. The hold is a constant
  found by measurement: a shorter one returns memory that is about to be reused and pays for
  it in faults, and a longer one keeps the peak. It stays a constant because the two ways of
  doing better that were built did not. A hold that learned a length for each region halved
  the faults of a program that takes its blocks back a second after freeing them, changed
  neither its time nor its tail, and cost a query engine a tenth of its peak. A long hold
  under which any growth pays for itself, by returning as much free memory elsewhere, kept
  that peak only to within the cap and removed fewer faults, for an account of every way
  memory becomes resident (`docs/research/large-blocks.md`, "Free slots: a hold, and why no
  hold serves everyone"). The faults a short hold costs are cheap, and the peak a long one
  costs is not. `maintain` takes the same bounded looks once an interval, and in a silence
  returns held slots at its second round.
- **Under a floor, the clock returns nothing.** The floor (see "Sizing the
  caches") is measured against what a pass could return: resident pages with
  nothing live on them, and while the program runs only those above the frontiers. While
  that totals no more than the floor the clocked pass returns
  nothing, and above it a pass stops at the floor. It is not measured against all the free
  memory the heap retains. A large heap holds many times the floor in free slots on pages
  with live neighbours, which nothing can return, so by that measure the floor would never
  apply to it, and the last megabytes a pass could take are the ones the program uses
  next. Each lap of the pass counts what it saw, and the next is held to that. A few tens
  of megabytes kept by a process is a price we pay freely (see "Goals"); returning them
  buys the process nothing it will notice, and costs it the calls and then the faults when
  its next burst arrives. `collect` is not floored.
- **Retained memory is counted, and growth pays for it.** The back end keeps an estimate
  of the free memory it holds that is still resident, small and large classes together.
  Frees and laundered magazines raise it; only reuse and purging lower it, and it is never
  reset. A free never returns memory because the count is high. A level cannot tell idle
  memory from memory that is about to be reused, and a program that frees and reallocates
  in the same classes would fault back whatever its frees returned; only the clock can tell
  what is idle. What a level is good for is a bound. The address space is divided by class,
  so memory freed in one class can serve another only by way of the OS, and a program that
  frees a great deal of one size and then allocates as much of another would hold both. So
  while the count is over a cap, growth is paid for: memory carved past a watermark is
  matched by an equal purge of retained memory, from wherever the purge walk stands and
  whether or not fills still reach it. The carve returns one bounded step of that and
  leaves the rest owed; the heartbeats that follow pay it a step each, and a debt whose
  walk finds nothing to return is dropped rather than walked for again. A heartbeat pays
  nothing while a clocked pass is under way: the pass is already returning everything over
  the floor that the frontiers allow, and the debt waits for it to end.
  This is the one place a frontier is ignored: the program is growing now, and holding
  both is what the cap exists to stop. Taking back pages that a purge returned is reuse,
  not growth, and is not charged; charging it sets the allocator purging what it is about
  to refill. That leaves a class that the clock had emptied free to refill while another's
  freed memory is still resident; the clocked pass returns what fills no longer reach, so
  that lasts a few intervals. Free objects on pages that
  also hold live ones cannot be purged; that is fragmentation, and it stays counted. A
  large heap can therefore sit over the cap for good. Its growth always pays, and what it
  has stopped using goes back by the clock, but the memory it is reusing is left alone.
  The cap plays no part in the clocked pass: over the cap as under it, a
  pass takes only what lies above the frontiers and stops at the floor of what it could
  return. Only growth goes below a frontier. Objects
  parked in magazines, spares and the depot are not free and are not in the count; the
  next rule handles them.
- **Parked objects do not pin the heap.** An object parked in a magazine holds its page as
  surely as a live one, and parked objects are a random sample of wherever the heap has
  been. Four rules keep them moving back to the bitmaps, where the next fill takes the
  lowest free addresses instead.
  - A magazine does not stay loaded for more than an interval. A slow path looks at one
    class of its own slab in turn. If the magazine loaded there has been loaded for an
    interval, the slow path takes it with the routine that replaces a magazine and launders
    it. This needs no fence, because the slab is the thread's own. A class in use pays one
    refill per interval. A class that fell out of use on a CPU that is still busy is
    emptied, which the idle check, being per slab, cannot do.
  - Full spares do not stay either. Once an interval each CPU sweeps its own slab, a class
    per heartbeat, taking that class's whole stack of full spares in one step, laundering
    one and sending the rest to the depot. Spares need their own rule: on a CPU that only
    frees a class, the loaded magazine is always fresh and nothing ever pops a full spare.
  - A depot stack sheds what it did not need. The depth a class's stack never fell below
    during an interval is laundered, one magazine per heartbeat, whichever class the slow
    path was serving.
  - One retiring full magazine in every few is laundered instead of kept, so objects in
    steady circulation are gradually replaced by the lowest free ones. How few follows
    evidence. Each lap of the purge walk measures the small classes' free slots on pages
    that also hold live objects, as a share of what those classes have carved: that is
    what a higher rate reduces, and nothing else can. From a share of 1 in 64 the rate
    doubles from one retirement in 32, and again at 1 in 32 and at 1 in 16, so a heap
    that a fall in its short-lived objects has left fragmented launders one retirement in
    four, and a heap in steady use, whose share stays well under the first step, pays
    nothing. The rate steps back down once a lap as the evidence goes, and a pass that
    finds no more than the floor retained steps it down too, so it falls in a quiet
    program as well (`docs/research/memory.md`, "Refresh on evidence").
- **Huge pages only where objects are dense.** A region starts marked against transparent
  huge pages, so a lightly used class costs only the 4 KiB pages it touches. Two things
  count as evidence of dense use. A class of objects of at most a kilobyte, several to a
  page, that has carved enough for one huge page to be a small fraction of it has the rest
  of its region marked for huge pages, and further growth faults 2 MiB at a time. The
  small classes above a kilobyte never get huge pages: a block of a page or so is often
  touched in part, and each part touched would cost 2 MiB. A large block that grows, by doubling or less
  and past the size of a huge page, with the last piece it wrote at least half full of
  data, is being appended to. The part of its slot up to the size it asked for is marked
  for as long as the block holds it; its free returns the slot whole and takes the mark
  back. A block that grows in place over a huge page it had only started has that page
  returned and its first bytes written back, so that the page faults whole instead of a
  small page at a time; a page whose remainder is already resident is left alone. A
  large block allocated at its size in one call gets no huge pages: it is often touched
  sparsely, each page touched would cost 2 MiB, and the kernel later fills in any marked
  range that is touched at all. This is a limit we accept. A program that fills such
  blocks densely and reads them at random, as a query engine does its hash tables, loses
  a few percent of its time to page walks; servers lose nothing we can measure, because
  their walks are on small objects. A wrong guess costs far more than a right one gains: a
  block touched sparsely holds many times its memory, and on a machine short of free huge
  pages the faults stall. Telling the two apart needs a look at the block while it lives,
  which would put a second writer on a slot the design gives to its holder alone
  (`docs/research/large-blocks.md`, "Huge pages only for blocks that prove dense";
  `docs/research/program-effects.md`, "TLB and huge pages").
- **Split a huge page only where fills have stopped reaching.** Returning part of a huge
  page costs the page its mapping, so below a region's frontier, in its huge-page part,
  purging works only in whole 2 MiB units. That includes the unit holding the watermark,
  whose upper part was made resident by the fault and never allocated: a purge first carves
  the rest of that unit and frees it, counting it as retained, so the whole unit is ordinary
  free memory that it can claim like any other. Above the frontier whole units are not
  enough. Fills take the lowest free slots, so when a class's demand falls its short-lived
  objects pack into the bottom of the region, and the top is left a few long-lived objects
  to a huge page, each holding 2 MiB. A program whose sizes shift between phases kept most
  of every trough that way (`docs/research/memory.md`, "When the size mix wanders, huge
  pages ratcheted"). So a step that may return memory treats the huge pages above the
  frontier as small pages. One that is wholly free goes back whole. One with a quarter of
  its pages or more free and resident is split, and its free runs are returned. Under a
  quarter, what would go back is not worth the mapping, and the huge page stays whole. A
  sample of a few pages, at places that move with every lap, chooses which huge pages are
  counted, so a lap over dense huge pages reads little more than it would without the rule.
  The purge asks the kernel to split a huge page before it returns part of it: pages
  returned from a huge page that is not split leave the resident set but stay charged to the
  cgroup until reclaim splits it, and the thread that meets the limit pays tens of
  milliseconds for that. A split costs a slow path what a step of returns does, and counts
  as one.

  The advice follows what went back. Under the advice the kernel rebuilds any huge page
  with one page in use, so when a pass is done with a region it takes the advice off, in
  one call, from the lowest huge page it returned part of upward, or from the frontier
  where fills have since passed that page: never below the frontier as it then stands. It
  takes none off where nothing went back: a change of advice takes the address-space lock
  for writing, and a server whose classes wobble paid for needless ones in its tail. Each
  region keeps that boundary in one word. Above it a split huge page is small pages to
  every pass, and one that never had a part returned is still whole and is judged as below
  it. A claim that reaches above the boundary marks the word, and the background thread
  puts the advice back on, up to the frontier and a few huge pages beyond; where there is
  no thread a heartbeat does it, one region a step. The headroom is enough that a class
  growing back does not move its boundary many times an interval, and no more, because
  advice over huge pages that stay sparse brings their memory back. It is a constant: two
  rules that sized it from the frontier's own movement did no better. A raise counts as a
  fill's reach over its range, so the next pass does not take straight back what the raise
  has just covered. A split huge page that fills reach again is small pages until the
  kernel rebuilds it, which costs a program that swings by gigabytes every few tens of
  seconds about a percent of its time.

  Under the advice the walk does not believe its own record of which pages have gone back.
  The kernel may have rebuilt a huge page whose free pages were returned, and those pages
  are then resident with nothing to say so. So there a free page of a huge page that holds
  anything is taken for resident, and a pass that returns such a huge page's free runs
  takes them all; the lowering that follows puts them where the record is true again.
  For a run the record calls returned, the pass first asks the kernel whether any of it is
  resident, once it holds the run's slots, and leaves a run that is not: one cheap call in
  place of a split and a return that would do nothing. A kernel that will not say gets the
  run returned. While lowerings are refused the boundary cannot come down, so the record is
  believed again for as long as the refusal stands (see below), and the same runs are not
  returned over and over.
  A huge page with nothing in it cannot be rebuilt and is left alone. Counting those pages
  as resident when the advice goes back on was built first and was wrong: the kernel takes
  minutes to rebuild anything, so the count ran tens of megabytes over what was resident,
  enough to put a heap over the cap and set its growth purging memory in use. What the
  kernel has rebuilt is uncounted until fills reach it or a pass returns it, which on a
  real server lasted seconds.

  One thread changes a region's advice at a time. It takes an owner bit in the word with
  one compare-and-swap, makes the call, and writes the new boundary as it gives the bit
  back; a thread that finds the bit taken leaves its mark or its note in the word and goes
  on. An owner that has not come back after two intervals is taken over, and the new owner
  first makes the kernel's advice agree with the word. So does the next owner after a call
  the kernel refused, which may have acted on part of its range. A kind of call the kernel
  refused is not made again before the next interval, or in a silence the walk's next lap,
  so a program at its limit of
  mappings, or with its memory locked, pays a bounded number of failed calls and keeps its
  huge pages whole. `collect` splits whatever it can return, wherever the frontiers stand.

`collect`, and `malloc_trim` in C, do all of this at once without waiting for the clock:
drain every slab in use, launder every depot stack, and purge what is free. A program that
knows it is idle calls it to return what a quiet process would otherwise keep.

Every rule here returns memory with the one call, and the kernel refuses that call on
memory the program has locked (`mlock`, `mlockall`). A process with its memory locked
therefore gets nothing back from the allocator: free memory stays resident and stays
counted, and each rule that would return some makes its call, fails, and goes on. One
case costs more than that. A freed block larger than a region gives its regions back only
once its memory has been returned, because whoever takes them next is promised zeroes, so
with memory locked those regions are never used again and their memory stays locked. This
is a limit we accept.

Memory at or above the watermark is zero, and so is purged memory. For large classes a
second bitmap records which free objects are still zero, so zeroed allocation skips the
`memset` when it can. Purging sets those bits for objects wholly inside the purged range,
before it releases their free bits. Small classes do not track this; zeroing a small object costs less
than the bookkeeping.

A zeroed request whose slot is not known to be zero clears it, and clearing makes every page
resident whether or not the program goes on to write there. A program that asks for a large
zeroed buffer and writes a little of it, again and again, then holds all of it for nothing.
Returning the pages instead of clearing them would fix that program and hurt another: one
that writes its whole buffer would pay a page fault for every page it used to find resident,
which costs more than ten times the clearing. No size separates the two, and one size class
can hold both at different times, so the free decides from evidence. A zeroed request of
16 MiB or more that is answered from a slot known to be zero, so that nothing was cleared
and none of it is resident, is marked. When it is freed, the free asks the kernel about one
page in each sixteenth of the block. If at most one of them is resident, the tenant left the
block mostly untouched: the slot goes back to the kernel whole, at once, and is recorded as
zero and not retained, so the next zeroed request for it clears nothing either. Otherwise it
is an ordinary free. Returning such a block is cheap for the reason it is right: almost
nothing of it is resident. A block that was cleared with `memset` is never marked, so a
tenant that writes what it is given, on a slot that stayed resident, pays what it always
did. A request that is not zeroed is never marked either: a scratch buffer written in small
part and taken again within the hold costs nothing as it is, and would fault on every life.
What the look finds chooses between two valid frees and nothing else, so it does not have to
be right. It sees pages touched, not pages written, so a block the program read widely
counts as used, and it can miss a written run shorter than an eighth of the block. Below
16 MiB the look would be a visible share of a block's life
(`docs/research/large-blocks.md`, "Zeroed blocks: evidence at the free").

### Large objects

Large classes have no magazines. Allocation and free operate on the region bitmap directly,
at the cost of a few atomic instructions, which is small next to the cost of touching the
object.

A large object may be given a slot larger than it asked for, which is why its class is read
from its address. `realloc` and `grow` succeed in place whenever the new size fits the slot.
When an object outgrows its slot it moves to one with several times the room, so a buffer
that doubles repeatedly is copied a few times in total, not once per doubling. The address
space this spends is plentiful, and the physical memory is only what the program touches.
An object larger than a region takes a run of whole regions.

A large object does not start at the start of its slot. Slots of a class are all aligned
alike, so objects that began there would put their cache lines in the same sets, and a
program that walks many such buffers together would miss the cache on every access. An
object whose alignment is at most a cache line starts a multiple of 64 bytes into its slot,
chosen by the slot's index, and is given a page more room so that any such start fits.
Nothing is stored: a block starts at its slot's start or at that offset, and any other
pointer into the slot is not a block.

The current size of each large object is recorded out of band, one word per slot, written
only when the object is allocated, resized or freed. A free leaves the size there for the
slot's next claim, and adds when the free happened. C's `realloc` needs it to know how much to
copy, and the usable size must not be the whole slot: a caller that fills what it is told it
may use would touch many times what it asked for.

A Rust resize stays in place only when the caller's next `dealloc` will still find the right
class. For a small object that means the new size maps to the same class. For a large object
it means the new size is still large and fits the slot. Anything else moves the object.

A large object that has to move because it grew moves into a slot with room to grow in
place. A heap with its full reservation gives sixteen times the block's size where that
slot is at most a sixteenth of a region; otherwise, and in every heap that got less than
its full reservation, the room is four times where that fits a region, and none past that.
Each falls back to the next when the larger slot cannot be had. A grow that must hand
back zeroed memory takes no room, so that it never clears what nobody asked for. A buffer
that doubles is otherwise copied at every
doubling, and the copies add up to more than the buffer. The room is given only where a
region still holds many such slots, because a region is never taken back from a class: a
class with one slot to a region would let a few thousand growing buffers use every region.
A heap under an address-space limit has small regions and none to spare, so it does not
make the trade at all. The room costs address space and
nothing else: pages nobody wrote are not resident, and are never counted, kept or purged
(see below). Under strict overcommit it does cost commit charge, since a region's writable
prefix is charged whole, and a failed commit falls back to less room. A factor only makes
copies rarer. A buffer still copies itself once each time it outgrows its room, so how much
a given final size has copied depends on where it falls between two moves.

Shrinking gives memory back. A large object that shrinks in place returns the whole pages
past its new size at once: nothing else can use them while it holds the slot, so otherwise
they would stay resident until it is freed. One that shrinks to a small enough fraction of
its slot moves instead, because a region has few slots of a large class, and a slot should
not be held for good by an object a tiny fraction of its size. The fraction is half of what
a block fills when it moves into a slot of that size with room: a thirty-second for the
small slots of a full heap that give sixteen times, an eighth for every other slot and for
blocks of a region or more. Being below what a block fills when it moves in, it lets a
block resized back and forth across a class boundary settle in one slot instead of being
copied on every call.

When a large object has to move, the copy goes in pieces. Each piece's pages are tested
first. A piece with no all-zero page is one copy. Where there are zero pages the copy leaves
them out, and the destination is purged only under them, so a block the program wrote
sparsely stays sparse. Unless the old slot is kept, as below, each piece's pages go back
behind it, so the two copies are never both resident.

What a growing block leaves behind is a choice between two cases that pull against each
other. A buffer grown and freed again and again should find its memory resident each time;
many buffers growing at once should not leave a trail of the blocks they outgrew.

A grown block that is freed is an ordinary free, unless it carries huge-page advice, when
its slot goes back whole (see "Returning memory and huge pages"). Otherwise its slot stays
resident, the hold decides when it goes back, and it is charged as any free is: a program
that grew one buffer is likely to grow another.

A slot that a block has outgrown is kept resident, whole, while the outgrown slots that
nothing has taken again total no more than 64 MiB, and goes back behind the copy beyond
that. This is the one place where a free acts on a level, and it may because of what the
level counts: only slots that a growing block left behind and that nothing has reused.
Where buffers grow one after another, each takes the slots the last one left and the count
stays small. Where many grow at once the slots pile up, and the count is the trail itself.
Counting anything wider does not work. The level was first set against all retained large
memory, which the hold keeps above its floor by design, so on any heap of size it was
always exceeded, and every outgrown slot went back and was faulted in again within
milliseconds (`docs/research/large-blocks.md`, "What growth leaves behind"). The bound also
limits how much the two copies of one moving block can hold between them. The purge falls
only on a block the program copied to reach its size, so it costs at most about what the
program has already spent on that block.

A kept slot carries a mark in its size word. The count rises by the slot's charge when it
is kept, and falls by what is removed when a claim takes the slot or a purge returns its
pages. It is exact while nothing races and a hint when something does: a wrong value costs
a keep or a purge, never a slot in use. A slot under huge-page advice is not kept. Its
pages come back two megabytes to a fault, which costs less than writing over pages that
have gone cold. Nor is one that still carries the mark of an unwritten zeroed block.

A move for any other reason, such as a block shrunk far below its slot, returns its pieces
behind the copy and then frees the old slot as a free of that block would.

A free slot is charged, against retained memory and against that count where it applies,
the pages that may be resident, not its whole size: a block that moved in with room to grow fills a small
fraction of its slot. The record is the slot's clean-page bits, and a clean bit is true: it says
the page has not been written since it was carved or last purged. Only a purge sets one, and
the carve of a slot for the pages past the block it was carved for. A claim, and anything
that widens a block, clears them under what the block may write. A free writes none. So a
smaller tenant after a larger one is charged what the larger one left, and the free, the
claim, the keep decision and the purge count a slot by the same bits and cannot disagree.

That leaves the pages a larger tenant wrote past a smaller one's block resident inside an
occupied slot, where no purge reaches them, since a purge claims the slot first. In a slot
that tenants of many sizes keep taking, what is resident would be the high-water mark of
them all. So the claim of a free slot returns what lies past both its new block and the
last tenant's, whose size the free left in the size word: from the top down, and one purge
step at most, as every return on a request's path. The last tenant's room is kept, because
the next may need it; what earlier tenants left goes, a step at each claim. Only the
claimer does this, before it hands the block out, because nothing but its holder touches
an occupied slot. What a trim has not reached stays resident, and uncounted until the
slot's free charges it. The price is page faults when a later, larger tenant needs the room
back: a program that churns large buffers of many sizes gives a few percent of its time
for a lower peak (`docs/research/large-blocks.md`, "Slack: what earlier tenants left in an
occupied slot"). Where the kernel refuses to take memory back, as it does for locked memory,
each such claim makes one call that fails.

Moving the pages with `mremap` instead would copy nothing, but it splits the heap's
mapping in ways that take several more rules to undo, and it adds nothing to the memory
saved; we copy.

### Interfaces

- **`GlobalAlloc`** for `#[global_allocator]`. `dealloc` of a small object is the free
  routine and nothing else.
- **`Allocator`**, on Rust versions that have it, through a zero-sized handle. All handles
  share the one heap. `allocate` returns the full class size of a small block and the
  usable size of a large one. A zero-size request gets a
  dangling, aligned pointer and no block.
- **C ABI**: `malloc`, `calloc`, `realloc`, `reallocarray`, `free`, the aligned-allocation
  family, `malloc_usable_size`, `malloc_trim` and the sized frees: every function glibc
  exports that takes or returns a block. `free(p)` reads the class from the address table
  and joins the same path as a Rust free. A pointer outside the heap is a caller's bug and
  aborts the process. A pointer into the middle of a small object is the same bug, but the
  fast path does not look for it: finding a slot's start takes a multiplication per free.
  Such a pointer is caught only if it reaches the bitmaps before it is handed out again.
  `malloc_usable_size` reports the class size of a small object and,
  for a large one, its recorded size extended to the end of the page it ends in. Callers write into the room
  they are told they have, so a `realloc` that moves a block copies that much and `calloc`
  zeroes that much.
  `realloc(p, 0)` frees `p` and returns null, as glibc does.
- **C++ operators are not exported.** A C++ program's `operator new` and `operator delete`
  stay its C++ runtime's, which calls our `malloc` and `free`; that is correct in every
  arrangement of libraries. Exporting the operators is sound only if we export all of them:
  with our `delete` forms but another library's `new`, as when a garbage collector's or an
  engine's C++ library defines the operators, blocks reach us that are not ours. And we
  cannot export all of them: a throwing `new` that fails must raise `std::bad_alloc`, and an
  exception cannot unwind through the allocator's Rust frames. The price is a call layer for
  C++ and the size that sized deletes carry; `docs/research/roofline.md`,
  "C++ new and delete without our operators", measures it. An implementation of every form
  but the throwing ones was built and then removed; the last commit that has it is
  `58bf936`, under `crates/cralloc/src/c/`.
- **A size from C is a prediction, never trusted.** A sized free may use the size to find
  the class early, but the class from the address is the truth, and a wrong size behaves
  exactly like a free with no size.
- **`collect`**, a Rust function and what `malloc_trim` calls: return everything that can be
  returned now (see "Returning memory"). `inspect` reports where the heap's memory is,
  without changing it.
- **`maintain`**: one round of the clocked maintenance for the whole heap. The background
  thread calls it; a program with an idle hook may call it instead.

A macro exports the C names from the crate that invokes it; a cargo feature does the same,
and the `LD_PRELOAD` library always exports them. A feature alone would not do: cargo
unifies features across a workspace, and every test binary would then replace `malloc`. The
C layer sets `errno` through the C library's accessor. In a build without the background
thread, that and glibc's `__rseq_offset` are cralloc's only references to a C library; on
musl, where there is no `__rseq_offset` to refer to, they are `errno`'s accessor and the
three pthread key functions that run code at thread exit. The background thread, which is
in the default build, adds what creating it takes: the library's calls to create and
detach a thread, to read and set the signal mask and to ask about the signal stack, and
on glibc its record of whether the process has more than one thread. The preload library
also asks the loader for the C library's own `pthread_create`, and if it cannot be found
the program's calls to create a thread fail.

The recommended build is `lto = "fat"`. Thin LTO also inlines the fast path into dependency
crates, but it silently stops doing so when the crate that declares `#[global_allocator]`
contains a private `#[used]` static or module-level assembly; declaring the allocator in a
small crate of its own avoids that.

C may free, resize or measure anything Rust allocated. Memory obtained from the C entry
points must be released through them, which Rust's own rules already require: its class may differ from
the one a Rust `Layout` of the same size implies.

### Process lifecycle

- **Start.** The static slabs are zero, so the first call takes the slow path, which
  reserves address space and publishes the heap with one compare-and-swap. Threads may race
  to do this; the losers unmap their reservations. Only a few full reservations fit in the
  address space at once, so a reservation that fails while another thread's is in flight
  says nothing about how much the process may have: the thread looks for the published heap
  and tries again at the same size. A failure with no other reserver in flight, or a bounded
  number of failed tries, makes it settle for smaller regions or report failure. One word
  counts the reservers in flight.
- **Unloading.** A shared object that contains the allocator must not be unloaded. A
  thread's rseq area goes on naming a sequence inside it after the last call returns, and
  the kernel reads that the next time the thread is preempted, so unloading kills the
  process. The preload library is linked so that it cannot be unloaded. Anyone else who
  builds the allocator into a shared object links it the same way; we do not guard against
  it at run time.
- **Without rseq.** A thread with no rseq registration reads a negative CPU number, so every
  routine sends it to the slow path and it never touches a slab. That covers a whole process
  under valgrind or with the C library's registration turned off, and on musl a single
  thread whose registration a sandbox refused. The slow path then serves small objects the
  way it always serves large ones, straight from the region bitmaps with atomic
  instructions. This is the same code, so it adds no second implementation, and it costs
  tens of cycles per call instead of a few.
- **Under Miri.** Miri runs neither assembly nor system calls, so the allocator's handle does
  not exist there. A program that is checked with Miri declares its global allocator only
  when not under Miri and keeps Miri's own, which checks each of its blocks individually.
  Our own tests run the allocator under Miri with the twins and the simulated OS.
- **Threads.** A thread has no allocator state. Starting and exiting need no code, and
  objects cached under a CPU stay with that CPU. The one exception is the next item.
- **Threads on musl.** musl registers no rseq area, so the allocator defines one per thread,
  in thread-local data emitted with the routines, at a page offset chosen to stay clear of
  the slab words. The area reads a negative CPU number until the thread's first slow path
  registers it with the OS. musl frees a thread's storage before the thread is gone, and a
  thread that dies with its area registered can take the process with it, so a pthread key's
  destructor unregisters the area and marks it retired. A retired or refused area reads a
  negative number for good, and the thread is served as any thread without rseq is. Three
  limits follow. The process's first registration makes the key, which takes a lock inside
  musl, so a signal handler that makes the process's first allocator call while its own
  thread is inside musl's key functions deadlocks. A thread whose first allocator call
  comes after the destructor can no longer run, from another key's destructor in the last
  round, registers and dies registered, with the risk above. And the destructors that run
  after ours, Rust's thread-local destructors among them, free on the slow path.
- **Idle caches.** Memory can sit in the slab of a CPU the process no longer runs on. Each
  slab has an activity counter that its slow paths advance, and a heartbeat that finds one
  unchanged for an interval drains that slab. The interval is the one under "Heartbeat":
  time, read from the CPU's cycle counter without a system call. It must not be measured in
  heartbeats: those come faster the harder the process allocates, and a slab that is busy on
  fast paths alone would be drained over and over. Such a slab still looks idle, and is
  drained once per interval, which costs it one refill. Every process has a **generation**,
  a random number; its marker is the
  generation shifted above the low 19 bits, with the lowest bit set.
  1. Claim the slab by writing this generation into its drain flag with a compare-and-swap.
     If the flag already holds this generation, another thread is draining; stop.
  2. Exchange each class's slab word and each spare-stack head with this generation's
     marker, one atomic instruction each, keeping the old values.
  3. Ask the OS to restart every sequence in flight on that CPU (`membarrier`). If the call
     fails, put each old value back with a compare-and-swap against the marker, release the
     claim and stop.
  4. Read each word again. If it is still the marker, no thread can commit to it, and the
     drainer owns the old value: it stores zero and, if the old value named a magazine,
     sends it to the depot. If it is anything else, a sequence that started before step 2
     committed, the old value still belongs to that CPU, and the drainer takes nothing and
     writes nothing.
  5. Release the claim.

  The routines on the fast path treat any marker as "no magazine" or "empty stack" and never
  store over one. So only a commit from a sequence already in flight can replace a marker,
  and step 3 ends all of those. The claim means a slab has one drainer at a time, so a
  marker a drainer finds in step 4 is its own. This is the only time one CPU's
  magazine words, a slab's hot words and spare-stack heads, are written from another. The
  slab's other words are hints, as "Concurrency rules" says, and one of them, the idle
  check's record of when it last saw the slab's counter move, is written by whichever CPU
  looks. Start-up registers for the `membarrier` command; where the OS
  refuses, draining is off.
- **Fork.** The child gets a copy of a heap in which every shared structure is valid at
  every instant. Objects that another thread held privately at the moment of the fork are
  lost to the child: a small, bounded leak. A drain that was under way leaves a claim and
  markers with no drainer. The generation lives on a page that is wiped on fork, and the
  child's first slow path draws a new one, so the child sees the parent's claims and markers
  as belonging to another generation and treats them as absent. A change of huge-page
  advice that was under way leaves an owner bit whose thread the child does not have, and
  advice that may not match the region's word. That same slow path clears every such bit
  and has each region's next change make the kernel's advice agree with its word first.
  A region that a thread of the parent had named for its class, and not yet marked as
  named for the walks (see "Address space"), would stay hidden from the child's walks for
  good, so that slow path also marks every region a class is carving or has carved from.
- **Signals.** A handler may allocate and free. A signal aborts any sequence in flight, and
  the slow paths hold nothing that a nested call could trip over. The exceptions are the
  places where the allocator calls the C library, which has locks of its own: the creation
  of the background thread (see "Heartbeat" for the handler its test cannot see), and on
  musl the first registration of the process (see "Threads on musl").

### Concurrency rules

These hold everywhere, and the proofs and tests lean on them.

1. A slab's magazine words, the hot word and the two spare-stack heads of each class,
   change only through an rseq routine that commits with one store, or through the drain
   steps under "Idle caches". The slab invariant is about these words and no others.
2. Other shared state changes only through single atomic instructions. That includes the
   rest of a slab: its counters, stamps, sweep position and stock words. They are hints,
   kept with a plain load and a plain store by whatever slow path is on that CPU, outside
   any sequence. A thread that moved, or was preempted between the load and the store,
   loses or misplaces an update, and each hint is one whose wrong value moves a launder, a
   drain or a magazine earlier or later and does nothing else.
3. No locks, no waiting, no thread-local state. The rseq area that the OS's interface
   requires of each thread is an exception, and on musl it is ours. The background thread
   is another: it sleeps on a word of its own, and no request ever waits for it. And where
   the allocator calls the C library (see "Interfaces") it is subject to that library's
   locks.
4. Between any two instructions of any slow path, shared state is valid and anything
   unreachable from it is owned by exactly one thread.
5. All-zero is a valid state for every structure.

### Repository layout

| Path | Contents |
|---|---|
| `crates/cralloc/` | The allocator, one module directory per component. |
| `crates/cralloc-preload/` | The `LD_PRELOAD` library. |
| `lean/` | The proof framework, the routines and their proofs, and the generator that writes the assembly into the allocator crate. The framework derives from verified-garbage, a project that proves assembly implementations of cryptography in Lean. |
| `bench/` | Roofline harnesses, baselines and workloads. Not part of the workspace. |
| `tla/` | TLA+ models of the lock-free protocols. |
| `fuzz/` | Fuzz targets. |
| `docs/design.md` | This document. |
| `docs/research/` | Measurements and prior-art notes behind the design. |
| `docs/safety/` | The safety ledger. |
| `third_party/` | Local reference checkouts. Not part of the repository. |

Components depend downward only: the interfaces on the front end and large objects, those on
the depot and back end, everything on size classes and the OS interface. The OS interface is
a trait with a Linux implementation built on raw system calls and a simulated implementation
for tests.

The crate's public API is small: the allocator's handle, the statistics it reports, the
background thread's controls, `abort`, and the macro that exports the C names. The components are private. The test kit, the fuzz
targets and the benchmarks reach them through a hidden cargo feature that is not part of the
API.

## Testing and Verification

### What is proved, and how

| Layer | Method | Strength |
|---|---|---|
| Assembly routines | Lean, against a model of the instructions and of rseq | Every input, every interruption point |
| Size classes, alignment, slot masks, bitmap word operations, address and layout arithmetic, packed words, the decisions of the clocked rules | Kani | Every input |
| The twins | Kani, against the routines' specifications | Every state of one small slab, with an interruption before any one step |
| These lock-free protocols as designed: the routines, magazine exchange, drain, depot stacks, summary bits, purge, the assignment of regions, the boundary of huge-page advice, start-up, fork | A TLA+ model of each, in which the OS aborts, moves, fences and forks threads, checked exhaustively with TLC | Every interleaving of the model, within small bounds |
| The Rust that implements those protocols | Schedule explorers in the test code, which run it unchanged under every schedule within a bound | Bounded schedules, on small heaps |
| The allocator as a whole | Model-based tests, Miri, fuzzing, and real programs under the preload library | Testing |
| `unsafe` blocks none of the above reaches | Safety ledger | Written proof, adversarially reviewed |

No proof covers a run of operations on a whole heap. Bounded model checking does not get
through the back end's nested retry loops, so that ground is held by the explorers and the
model-based tests.

The property all of this serves: every operation behaves as one atomic step of an abstract
allocator whose state is the set of live blocks. A returned block is aligned, at least as
large as requested, inside the heap, and disjoint from every live block. The allocator never
reads or writes the bytes of a live block except where the API says so, and never touches
memory it does not own.

### What is trusted

The proofs assume the following and nothing else about the platform. Each is exercised by
tests where a test can reach it.

- A thread that is preempted, migrated or signalled inside a registered sequence resumes at
  the sequence's abort handler.
- The CPU number in a thread's rseq area is the CPU it is running on, changes only at an
  event that aborts its sequence, and is negative while the thread has no registration.
  Nothing else is assumed about its value.
- `membarrier` with the rseq command returns only after every sequence then in flight on
  the named CPU has completed or been aborted.
- glibc registers a thread's rseq area before the thread can call the allocator. On musl,
  where the area is ours, it reads a negative CPU number until registered, and the OS
  accepts one registration per thread.
- On musl, a signal handler that allocates does not make its thread's first call to the
  allocator at the one instruction boundary between a routine's arming store and its read of
  the CPU number. There the handler's slow path would register the thread under a sequence
  that began unregistered, and that one sequence would run to its commit unprotected. glibc
  registers before any call, so this cannot arise there.
- Nothing with the privilege to do so places a probe inside a sequence, which would let
  the thread return to it without an abort.
- Of a thread's attempts at a sequence, one is at last left to finish. Only termination
  rests on this.
- Fresh anonymous memory, and memory after `MADV_DONTNEED`, reads as zero.
- The instruction model, the Rust compiler and assembler, and Lean's kernel.

### Assembly

All assembly is generated from Lean; none is written by hand. A routine is a Lean term with
a specification and a proof, and the generator prints it as Rust source that is checked in.
A check fails if the checked-in assembly differs from what Lean generates, if any proof
uses an axiom beyond the standard ones or does not replay through Lean's kernel, or if an
out-of-line routine, once assembled, breaks the size and alignment rules given under "Front
end". The check is itself tested against forged proofs.

For each routine we prove four things. Run to completion, it performs its specified step on
the slab. Stopped before any instruction, including the last, it has changed nothing the
specification can see, and the registers the restart needs are intact. It reads and writes
only the thread's rseq area, the slab array and the magazine the slab word names. It
clobbers only the registers it declares.

The proofs assume, at entry and again at every retry, the part of the slab invariant given
under "Front end" that a routine can see: its class's word is zero, a marker, or a slot
address of a magazine of the class's size, and that magazine lies apart from the slabs, the
thread's rseq area and the stack. That nothing else refers to the magazine, and that its
pointers are distinct free objects, no routine relies on; the Rust slow paths keep the
whole invariant. It is stated once, in Lean. The layout constants the proofs assume are
emitted with the routines, and the Rust code is held to them.

The framework's own trusted core is the instruction semantics, the rseq interruption model,
the printer, the meaning of Rust's `asm!` operands, and each routine's stated contract.
Changes to these are reviewed separately from changes to routines.

### Models of the protocols

The protocols on which memory safety rests each have a TLA+ model in which the OS is an
actor: the routines under rseq, the exchange of magazines, the drain, the depot's stacks,
the summary bits, the purge as a claim, the assignment of regions to classes, the boundary
of huge-page advice, start-up and fork. In each the OS stops, moves and restarts threads, runs signal handlers, fences,
forks, and hands out address space. TLC checks every interleaving of a few threads over a few magazines and
objects, which is as many actors as the flaws these protocols can have need.
A configuration that is expected to fail is kept as a test: it shows that the model can see
a flaw, and that the rule which removes it is needed.

The other words that more than one thread writes have no model. A flaw in any of them
can cost memory returned late or work done twice. It cannot cost a block: whatever those
words decide, memory goes back only through a claim, and the claim is modelled. The
background thread's
handshakes, by which it parks or sleeps without losing a wake-up, are run under the
schedule explorer against a racing slow path. The words of the clocked pass, the debt
that growth owes, the hold's looks, and the hints that say which depot stacks owe or hold
magazines are covered by tests, and some of their decisions by Kani.

### Kani

Kani proves, for every input, the arithmetic that everything else stands on.

- The class function: the smallest class that fits, monotone, the same class for every size
  `Allocator` may pass back, alignment by construction, and C's 16-byte rule. Division by a
  class size without a divide. The two slot tests, which zero and every marker fail.
- The address space: the parts of a reservation fit and lie apart, the region and slot
  found for a pointer invert the slot's address, and slots lie inside their region and are
  aligned.
- The back end: the bitmap word operations, the metadata layout of every class, and, on the
  smallest geometry, exactly which slots a unit of pages touches.
- Every word that packs fields (a stack head, a slab word, a region's headers, the published
  heap) round-trips, every generation's marker is one the routines reject, and a magazine
  address the depot accepts lies inside committed magazine memory.
- The decisions of the clocked rules, as functions of the clock and the counts they read:
  when a purge pass starts and how much it may take, when the cap acts, when a magazine has
  been loaded too long, what a depot stack owes, when an idle slab is drained.
- The twins (below).

The harnesses that finish in a few seconds run in the merge gate, over the geometries in
use. The rest run on demand: every geometry the constructor accepts, and the exactness of
the multiplication behind the division.

### Twins

Kani, Miri and the schedule explorers cannot execute assembly. Each routine has a Rust twin
written to the same specification and selected by build configuration. A twin runs on a
simulated processor that the test controls: it says which CPU number the thread reads, and
it may interrupt the twin before any step, after which the twin starts again from the top as
the routine would. Kani checks each twin against its specification on every state of a small
slab (a word that is zero, any marker, or any slot of a magazine), with an interruption
before any one step.

A test compares each twin with its routine directly: it runs one and then the other from
the same scripted states of a slab and its magazines, and requires the same result and the
same memory afterwards. The routine is proved against the
specification in Lean, the twin is checked against the same specification restated in Rust,
and the heap's tests run the same bodies on the process's heap, with the routines, and on a
simulated heap, with the twins.

### Tests

- **Schedule exploration.** Explorers in the test code run the production code of a
  protocol, in the back end or the middle tier, on a few threads of which exactly one runs
  at a time. They switch threads only before an atomic operation and search, depth first,
  every schedule within a bound on preemptions. A schedule may also fork at any step, after
  which every thread but one is gone, and in the middle tier it may move a thread to another
  CPU or abort its sequence. The scenarios follow the TLA+ models and are checked for their
  properties. In the back end: no slot is handed out twice or lost, no free slot stays
  invisible, the first free after a fork heals what the fork broke, no purge returns the
  memory of a slot it does not hold, and a slot said to be zero is. In the middle tier:
  every magazine and every object has exactly one owner, no call waits, no marker or claim
  outlives its drain, and a fork duplicates nothing.
- **Interruption at every step.** Tests interrupt each twin before each of its steps in
  turn, alone, with a move to another CPU, and with a nested call that pops and pushes, and
  check that the interruption leaves no trace. The slow paths are interrupted at each of
  their steps in the same way: by a signal handler that allocates, by a migration, and by a
  fork that the thread does or does not survive. On the real routines, a mode of the soak
  harness (below) puts a hardware breakpoint on an instruction of one of the library's
  sequences; the trap makes the OS abort the sequence there, and its handler allocates and
  frees before the sequence restarts.
- **Model-based testing.** The test kit drives an allocator through streams of operations
  and checks every result against a shadow model of the live blocks: alignment, no overlap,
  usable size, zero fill, contents carried across a resize, and no write by the allocator
  into live memory. One model judges `GlobalAlloc`, `Allocator` and the C functions, on the
  process's heap and on a simulated heap: on one thread, on many with blocks handed between
  them, with threads that exit holding blocks, and in the child of a fork taken while other
  threads allocate. A failing stream is minimized to a script that replays. Allocators
  broken on purpose, one for each kind of bug the model claims to catch, keep the model
  honest: each must be caught through every interface. The back end has a model-based test
  of its own, over its slots and bitmaps.
- **Fault injection.** The simulated OS fails any call on demand, wholly or after acting on
  half of its range. A failure to obtain memory must surface as a null return and nothing
  worse; a failure to release memory must be survivable. Its memory is guarded where the
  host allows, so that touching memory the allocator has not committed faults as it would
  on Linux.
- **Miri** runs the allocator's own tests on a simulated heap with the twins, under strict
  provenance. The test kit's model builds for Miri on the same heap; the script that runs
  Miri runs the allocator's tests only.
- **Fuzzing** drives the same model from raw input, with a target for each interface.
- **The C functions** meet a table of edge cases. The C library is run against the same
  table, and broken implementations show that the cases bite.
- **Real programs** run with the preload library and must print and exit as they do without
  it: small C and C++ programs in the test suite, and on demand shell pipelines, git,
  compressors, interpreters and a cargo build. The library is checked to export no C++
  operator and to be marked against unloading. A soak runs a stress harness that checks the
  contents of every block, in modes that are hard on a per-CPU allocator: churn, hand-off
  between threads, fork, storms of short-lived threads, resizes, huge blocks, signals whose
  handlers allocate, idleness, trimming, and the breakpoints above. It runs them again on
  two CPUs with many threads, without rseq, and against a debug build, and it builds and
  tests several large programs with and without the library.
- **Probes** are small programs that check one assumption each about the kernel, glibc or
  the compiler, the ones under "What is trusted" among them: that a sequence can be aborted
  before any of its instructions, that no sequence commits after `membarrier` returns, and
  what a thread's rseq area holds from its first moment to its last.

The merge gate is deliberately small: formatting, lints, the tests and Kani harnesses that
take a few seconds, and a check that the generated assembly is what the generator last
wrote. The longer explorations and stress tests, the slow harnesses, the Lean check, TLC,
Miri, fuzzing, the preload checks and the benchmarks run on demand.

### Safety ledger

An `unsafe` block in code that ships is covered by a machine-checked proof where one is
practical. Where it is not, the block has an entry in `docs/safety/<crate>/<file>-safety.md`,
named after the source file without its `.rs`. An entry states the block's preconditions,
the invariants that make them hold, and a proof on paper. A fresh agent that did not write
the code reviews entries adversarially and records the commit it reviewed. A script in the
merge gate checks that every safety comment names an entry and every entry is named, and
lists the entries changed since their review; that list is cleared before a release. The
simulator, the twins and other test scaffolding are outside the ledger.

### Performance

Performance claims rest on three kinds of measurement.

- **Roofline microbenchmarks** compare each fast path with floor allocators (the cheapest
  code that could serve the workload), with variants that differ from the real path in one
  detail, and with other allocators. They report cycles and instructions from hardware
  counters, which do not depend on clock frequency.
- **Workloads**: multithreaded Rust servers and pipelines in which objects cross threads,
  compiler and data-processing runs, and established allocator benchmarks through the
  preload library. Rust workloads are measured with cralloc as the global allocator,
  because preloading discards Rust's sized frees.
- **Memory**: resident and huge-page-backed memory over time, fragmentation as resident
  bytes over live bytes, page faults, and how quickly memory returns after a peak.

Comparisons interleave the allocators, pin threads, repeat across process launches, and
include a run of one allocator against itself to expose a broken setup.
