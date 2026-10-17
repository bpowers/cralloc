# asmalloc design

## Summary

asmalloc is a memory allocator for multithreaded Rust programs on modern Linux. It serves
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
middle  depot: stacks of whole magazines, shared by all CPUs
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
(`docs/research/zen5-roofline.md`, `docs/research/baselines.md`) agree: when objects cross
threads mimalloc takes 1.5 to 2.7 times as long as jemalloc in all but the shortest one-way
bursts, and both are several times slower than the cheapest code that can do the same job.
The same baselines show where each incumbent is strong: mimalloc on building and dropping
large structures, because it faults in huge pages; jemalloc on memory use and on churn;
per-CPU tcmalloc on large objects that cross threads.

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
that would give each thread its own cache while threads are fewer than CPUs. asmalloc
indexes by CPU number instead, for three reasons. It is in every kernel that has rseq. A
thread without a registration reads zero as its concurrency ID, which is also a valid one,
so telling the two apart would cost extra instructions on every call. And current kernels
can briefly show a thread a garbage concurrency ID after a failed `execve`.

### What the hardware says

We measured candidate fast paths and the primitives around them on the reference machine
(AMD Zen 5, Linux 7.2). The full study is `docs/research/zen5-roofline.md`. The numbers that
shaped the design:

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
| first touch of a 4 KiB page; the same amount of memory inside a 2 MiB huge page | 566 ns; 119 ns |

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
   partly used memory over fresh memory. Wholly free pages beyond a cap go back to the OS,
   and the memory parked in caches is bounded.
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
7. **One platform, done fully.** asmalloc is developed and measured on x86-64, Linux 7.2,
   glibc 2.43 and Rust 1.99. The fast path needs the calling thread to have a registered
   rseq area, which Linux has offered since 4.18. A thread without one, as under valgrind,
   still works, slowly (see "Process lifecycle"). asmalloc never hands a request to another
   allocator.

## Non-Goals

- **Compaction or meshing.** Fragmentation is handled by size-class segregation and reuse
  policy.
- **Speed without rseq.** The mode that runs when rseq is missing exists for correctness
  under tools such as valgrind. It reuses the path large objects take and is not tuned.
- **Other architectures, C libraries and operating systems.** aarch64 comes after x86-64 is
  complete; its measurements and proofs must be redone. A C library that does not register
  rseq, such as musl, needs asmalloc to register its own area. The design allows both; neither
  is built.
- **Hardened-allocator features.** No guard pages, quarantine or randomization. Two
  protections fall out of the design anyway: there is no metadata in freed memory to
  corrupt, and a double free is detected when the object returns to the back end.
- **NUMA awareness.**
- **Heap profiling.**
- **Allocation in a `vfork` or `CLONE_VM` child.** Such a child shares the parent's memory
  but has no rseq registration of its own, so the fast path is unprotected there.
- **More than one instance per process.** Two copies of asmalloc, for example the preload
  library and a binary that links the crate, have separate heaps and must not free each
  other's pointers.

## Implementation

### Vocabulary

- **Class**: a size class. Small classes are cached in magazines; large classes are not.
- **Region**: 4 GiB of address space that holds objects of exactly one class.
- **Magazine**: a small aligned array of pointers to free objects of one class.
- **Slab**: the per-CPU block of state that names the magazine loaded for each small class.
- **Depot**: shared stacks of full and empty magazines.
- **Routine**: one of the assembly sequences generated from Lean.
- **Twin**: a Rust function with the same specification as a routine, used where assembly
  cannot run.
- **Carve**: take never-used objects from the end of a region.
- **Launder**: return the objects in a magazine to their regions' bitmaps.
- **Purge**: return the physical memory behind free objects to the OS.
- **Heartbeat**: the bounded maintenance done each time a slow path runs.

### Address space

At first use asmalloc reserves 16 TiB of address space, inaccessible and uncommitted. About
3970 regions of 4 GiB hold objects and the rest holds metadata. If the process cannot have
that much address space, as under an address-space limit or valgrind, the region size is
halved until the reservation fits, so the number of regions stays the same; below a floor
of a few megabytes per region the allocator cannot start. A region is
assigned to a class on demand and holds only objects of that class, at
`region_base + index * class_size`. A 4096-byte table records the class of each region.

Every pointer therefore has two properties that hold without per-object metadata:

- its class is `class_of[(ptr - heap_base) >> region_shift]`, one load from a table that
  fits in the L1 cache;
- its index within the region is `(ptr - region_base) / class_size`.

The Rust entry points do not need even that for small objects: they compute the class from
the `Layout`. The table serves the C entry points, large objects, and a checked build that
verifies the caller's `Layout` against the address. The scheme also removes external
fragmentation between classes, because any free slot in a region fits any request for its
class.

The price is that objects of different classes never share a page. A program that uses many
classes lightly has one partly used page per class, and each hot class needs its own TLB
entries. The huge-page policy below is built around this.

A region becomes writable in steps, each a single `mprotect` that extends the writable
prefix well ahead of what is in use. Under strict overcommit the commit charge is the
writable prefix, not the reservation.

The rest of the reservation holds metadata: each region's bitmaps at an address computed
from the region number, and an array of magazines. Memory that holds application objects
never holds allocator state.

### Size classes and alignment

A request is first rounded up to a multiple of its alignment. Classes are every multiple of
8 bytes up to 64, then four per doubling. The class of a request is the smallest class at
least as large as the rounded size.

Alignment needs no further work. Regions are aligned to their size and objects sit at
multiples of the class size, so an object is aligned to the largest power of two that divides its
class size. With four classes per doubling, the class chosen for a size that is a multiple
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

Each CPU has a slab. Its hot part is an array with one 8-byte word per small class; a
colder part holds the CPU's spare magazines and counters. The slabs are a static array with
an entry for every CPU number the OS can produce (8192 on x86-64), so the address of a slab
word is the CPU number shifted and added to a link-time constant, with no pointer to load.
Untouched slabs cost no memory. A routine compares the CPU number with the array length and
takes the slow path if it is out of range. That one compare keeps a thread with no rseq
registration, which reads a negative number, away from every slab, and it means memory
safety does not depend on what the OS writes there.

A **magazine** is a block of 8-byte slots, a power of two in size and aligned to its size.
Slot 0 is a link word used by the depot, the last slot holds the fill level while the
magazine is not loaded, and the slots between hold pointers to free objects. Magazines are
numbered blocks in one array in the metadata area; they are created by advancing a counter
and are never unmapped. Their size falls as the class size rises, from 256 slots to 4, so no
magazine holds more than about 128 KiB of objects.

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

Allocation of an object whose class is a compile-time constant has this shape (the exact
instructions are chosen by measurement and may change):

```asm
    mov   rdx, [rip + __rseq_offset@GOTPCREL]
    mov   rdx, [rdx]                  ; where glibc put this thread's rseq area
    lea   r10, [rip + descriptor]
    mov   fs:[rdx + 8], r10           ; arm: tell the OS which sequence we are in
start:
    mov   r8d, fs:[rdx + 4]           ; CPU number
    cmp   r8d, SLAB_COUNT             ; out of range, or no registration
    jae   slow
    shl   r8, SLAB_SHIFT
    lea   r9, [rip + SLABS + CLASS*8]
    mov   rcx, [r9 + r8]              ; next free slot of the loaded magazine
    test  ecx, MASK                   ; no magazine, or empty?
    jz    slow
    mov   rax, [rcx - 8]              ; the object
    lea   rcx, [rcx - 8]
    mov   [r9 + r8], rcx              ; commit
end:
```

An executable can read `__rseq_offset` with one load instead of two, because the linker
gives it its own copy of the variable; that saves a cycle per pair, and a cargo feature
selects it. A shared library must go through the indirection shown.

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

Each routine is generated in two forms. The inlined form is an `asm!` block in an
`#[inline]` function with the class and mask as constants. It becomes part of `__rust_alloc`
in the user's binary and, with LTO, of its callers. The out-of-line form takes the class in
a register and loads the mask from a per-class table; the C entry points use it, as do Rust
callers whose layout is not constant.

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

Each CPU keeps a few spare full and empty magazines per class, in stacks whose heads live
in the cold part of the slab and are updated by single-store routines. A thread that
oscillates around a magazine boundary swaps locally without touching shared state. The
spares of one CPU are limited by a byte budget.

Every step is one commit. Between steps all shared state is valid and whatever the thread
holds is private to it. This is what makes the slow path safe to interrupt with a signal
handler that allocates, and safe to abandon at any instruction in a forked child.

### Depot

The depot holds, per class, a stack of non-empty magazines, and per magazine size a stack of
empty ones. A stack head is one 64-bit word: a 32-bit magazine number and a 32-bit version
counter, updated by compare-and-swap. Magazine memory is never unmapped, so reading a stale
link is harmless, and the counter means a stale head is accepted only if exactly 2^32 pushes
and pops intervene during one attempt.

Exchanging through the depot is how objects move between CPUs. A thread that only frees
fills magazines and pushes them; a thread that only allocates pops them. The cost is two
atomic instructions per magazine and no work per object, however frees and allocations are
spread across threads. Each class has a target for the bytes held in its depot stack.

### Back end: regions and bitmaps

Each region has a free bitmap with one bit per object, and above it a tree of summary
bitmaps, 64 to 1, in which a set bit means "something below may be free". A region also has
a watermark: objects at or above it have never been allocated, and their memory has never
been touched.

- **Filling a magazine** searches the class's regions in address order. In each, it walks
  the summary tree to the lowest bitmap word with free objects and claims bits from it with
  one compare-and-swap. The objects of one word are adjacent in memory, so a refilled
  magazine is spatially dense. If no region of the class has a free object, the fill carves
  from the last region's watermark with one atomic add, and when that region is exhausted it
  assigns a new one to the class.
- **Laundering a magazine** groups its pointers by bitmap word and sets each word's bits
  with one atomic OR. If a bit was already set, the object was freed twice and the process
  aborts.
- **Summary bits are hints.** They may be set when nothing below is free, and they never
  stay clear when something is. A free sets the word's bits and then sets every summary bit
  above it that it finds clear. Whoever empties a word clears the summary bit and then reads
  the word again. A free does not assume that the first free into an empty word set the
  summary bit, because that thread may have vanished in a fork before it could.

The order of preference in a fill is the fragmentation policy: reuse free objects before
carving fresh memory, and take the lowest address first. Live data packs toward the start of
each region and the high end drains, which is where purging starts.

### Heartbeat

Every entry to a slow path does a bounded amount of maintenance before it returns: at most
one magazine laundered, one step of purging, and one look for an idle cache. There is no
background thread and no timer. Maintenance happens at a rate set by allocation activity, in
units small enough that no single call pays much. This is mimalloc's pacing applied to a
per-CPU design.

Laundering is not the common case. Objects circulate through magazines and the depot, and
reach the bitmaps only when a class's depot stack is over its target, one magazine per
heartbeat.

### Returning memory and huge pages

A huge-page fault delivers 512 pages for about a fifth of the per-page price of small
faults, and a page returned too early is faulted straight back. Four rules follow.

- **Purge late and from the top.** Purging claims the bits of every object that overlaps a
  range of pages, exactly as an allocation would. If all of them were free it returns the
  pages with `madvise(MADV_DONTNEED)`. Either way it then releases the bits. Nothing else
  can allocate the range while it is claimed, so no lock is needed. Purging starts at the
  high end of a region and is rate limited.
- **Retained memory is capped.** The back end counts the free memory it holds that is still
  resident, updating the count once per laundered magazine and per large free. When the
  count passes a cap, the call that pushed it there purges wholly free pages until it is
  back under, large objects first. Two things lie outside the cap and are bounded
  separately: free objects on pages that also hold live ones, which is fragmentation, and
  objects parked in magazines, spares and the depot, which the spare budgets and depot
  targets limit. A program that frees a great deal and then goes quiet keeps at most the
  cap plus those; `malloc_trim` and its Rust equivalent release what can be released.
- **Small pages while a class is small, huge pages once it is large.** A region starts
  marked against transparent huge pages, so a lightly used class costs only the 4 KiB pages
  it touches. Once a class has carved more than a threshold, the rest of its region is
  marked for huge pages and further growth faults 2 MiB at a time.
- **Never split a huge page.** In the huge-page part of a region, purging works only in
  whole 2 MiB units.

Memory at or above the watermark is zero, and so is purged memory. For large classes a
second bitmap records which free objects are still zero, so zeroed allocation skips the
`memset` when it can. Purging sets those bits for objects wholly inside the purged range,
before it releases their free bits. Small classes do not track this; zeroing a small object costs less
than the bookkeeping.

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

The current size of each large object is recorded out of band, one word per slot, written
only when the object is allocated or resized. C's `realloc` needs it to know how much to
copy, and the usable size must not be the whole slot: a caller that fills what it is told it
may use would touch many times what it asked for.

A Rust resize stays in place only when the caller's next `dealloc` will still find the right
class. For a small object that means the new size maps to the same class. For a large object
it means the new size is still large and fits the slot. Anything else moves the object.

### Interfaces

- **`GlobalAlloc`** for `#[global_allocator]`. `dealloc` of a small object is the free
  routine and nothing else.
- **`Allocator`**, on Rust versions that have it, through a zero-sized handle. All handles
  share the one heap. `allocate` returns the full class size. A zero-size request gets a
  dangling, aligned pointer and no block.
- **C ABI**: `malloc`, `calloc`, `realloc`, `free`, the aligned-allocation family,
  `malloc_usable_size`, `malloc_trim` and the sized frees. These are exported when a cargo
  feature asks for them, and always by the `LD_PRELOAD` library. `free(p)` reads the class
  from the address table and joins the same path as a Rust free. A pointer outside the heap
  is a caller's bug and aborts the process. The sized frees also take the class from the
  address, since a wrong size from C would otherwise corrupt the heap. `malloc_usable_size`
  reports the class size of a small object and, for a large one, its recorded size rounded
  up to a page. `realloc(p, 0)` frees `p` and
  returns null, as glibc does. This layer sets `errno` through the C library's own
  accessor; it is the only part of asmalloc that refers to a C library.

The recommended build is `lto = "fat"`. Thin LTO also inlines the fast path into dependency
crates, but it silently stops doing so when the crate that declares `#[global_allocator]`
contains a private `#[used]` static or module-level assembly; declaring the allocator in a
small crate of its own avoids that.

C may free, resize or measure anything Rust allocated. Memory obtained from the C entry
points must be released through them, which Rust's own rules already require: its class may differ from
the one a Rust `Layout` of the same size implies.

### Process lifecycle

- **Start.** The static slabs are zero, so the first call takes the slow path, which
  reserves address space and publishes the heap with one compare-and-swap. Two threads may
  race to do this; the loser unmaps its reservation. Under a tight address-space limit that
  race can leave the smaller of two reservations in place.
- **Without rseq.** A thread with no rseq registration reads a negative CPU number, so every
  routine sends it to the slow path and it never touches a slab. That covers a whole process
  under valgrind or with the C library's registration turned off, and a single thread whose
  registration a sandbox refused. The slow path then serves small objects the way it always
  serves large ones, straight from the region bitmaps with atomic instructions. This is the
  same code, so it adds no second implementation, and it costs tens of cycles per call
  instead of a few.
- **Under Miri.** Our own tests run the allocator under Miri with the twins and the
  simulated OS. A downstream program run under Miri gets Miri's built-in allocator, so that
  Miri can check each of that program's blocks individually.
- **Threads.** A thread has no allocator state. Starting and exiting need no code, and
  objects cached under a CPU stay with that CPU.
- **Idle caches.** Memory can sit in the slab of a CPU the process no longer runs on. Each
  slab has an activity counter, and a heartbeat that finds one unchanged for long enough
  drains that slab. Every process has a **generation**, a random number; its marker is the
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
  marker a drainer finds in step 4 is its own. This is the only time one CPU's slab is
  written from another. Start-up registers for the `membarrier` command; where the OS
  refuses, draining is off.
- **Fork.** The child gets a copy of a heap in which every shared structure is valid at
  every instant. Objects that another thread held privately at the moment of the fork are
  lost to the child: a small, bounded leak. A drain that was under way leaves a claim and
  markers with no drainer. The generation lives on a page that is wiped on fork, and the
  child's first slow path draws a new one, so the child sees the parent's claims and markers
  as belonging to another generation and treats them as absent.
- **Signals.** A handler may allocate and free. A signal aborts any sequence in flight, and
  the slow paths hold nothing that a nested call could trip over.

### Concurrency rules

These hold everywhere, and the proofs and tests lean on them.

1. A slab changes only through an rseq routine that commits with one store, or through the
   drain steps under "Idle caches".
2. Other shared state changes only through single atomic instructions.
3. No locks, no waiting, no thread-local state.
4. Between any two instructions of any slow path, shared state is valid and anything
   unreachable from it is owned by exactly one thread.
5. All-zero is a valid state for every structure.

### Repository layout

| Path | Contents |
|---|---|
| `crates/asmalloc/` | The allocator, one module directory per component. |
| `crates/asmalloc-preload/` | The `LD_PRELOAD` library. |
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
for tests and proofs.

## Testing and Verification

### What is proved, and how

| Layer | Method | Strength |
|---|---|---|
| Assembly routines | Lean, against a model of the instructions and of rseq | Every input, every interruption point |
| Size classes, alignment, slot masks, bitmap word operations, address arithmetic | Kani | Every input |
| Lock-free protocols: magazine exchange, drain, depot stacks, summary bits, purge, start-up | A TLA+ model of each, including the OS's aborts and fences, checked exhaustively with TLC; loom on the Rust that implements them | Every interleaving of the model |
| Back-end and slow-path Rust | Kani on bounded heaps over the simulated OS; model-based tests; Miri | Bounded, plus heavy testing |
| `unsafe` blocks none of the above reaches | Safety ledger | Written proof, adversarially reviewed |

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
- glibc registers a thread's rseq area before the thread can call the allocator.
- Nothing with the privilege to do so places a probe inside a sequence, which would let
  the thread return to it without an abort.
- Fresh anonymous memory, and memory after `MADV_DONTNEED`, reads as zero.
- The instruction model, the Rust compiler and assembler, and Lean's kernel.

### Assembly

All assembly is generated from Lean; none is written by hand. A routine is a Lean term with
a specification and a proof, and the generator prints it as Rust source that is checked in.
A check fails if the checked-in assembly differs from what Lean generates, or if any proof
uses an axiom beyond the standard ones.

For each routine we prove four things. Run to completion, it performs its specified step on
the slab. Stopped before any instruction, including the last, it has changed nothing the
specification can see, and the registers the restart needs are intact. It reads and writes
only the thread's rseq area, the slab array and the magazine the slab word names. It
clobbers only the registers it declares.

The proofs assume the slab invariant given under "Front end", at entry and again at every
retry, and leave it to the Rust slow paths to maintain. It is stated once, in Lean. The Rust
constants are generated from that statement, and the front end's debug validator is tested
against vectors evaluated from it.

The framework's own trusted core is the instruction semantics, the rseq interruption model,
the printer, and the meaning of Rust's `asm!` operands. Changes to it are reviewed
separately from changes to routines.

### Twins

Kani, Miri and loom cannot execute assembly. Each routine has a Rust twin with the same
specification, selected by build configuration. Differential tests compare routine and twin
over all small slab states, and compare both against vectors evaluated from the Lean
specification.

### Tests

- **Interruption at every instruction.** For each routine and each instruction boundary, a
  hardware breakpoint delivers a signal there, which makes the OS abort the sequence. The
  test checks that nothing was committed and that the retry succeeds, then repeats with a
  handler that itself allocates on the same CPU.
- **Model-based testing.** A shadow model tracks every live block and checks alignment,
  overlap, usable size, zero fill and content preservation across `realloc`. It runs
  single-threaded, multithreaded with blocks handed between threads, and under a
  deterministic scheduler in which the test input chooses which simulated thread runs on
  which simulated CPU and where a twin is interrupted. Deliberately broken allocators keep
  the model honest.
- **Fuzzing** drives the same model from raw input.
- **Fault injection** in the simulated OS. A failure to obtain memory must surface as a null
  return and nothing worse; a failure to release memory must be survivable.
- **Oversubscription and migration stress** on the real OS, to exercise the guarantees the
  design trusts.
- **Real programs** run with the preload library, and the C entry points run a conformance
  suite.

### Safety ledger

An `unsafe` block is covered by a machine-checked proof where one is practical. Where it is
not, the block has an entry in `docs/safety/<crate>/<file>-safety.md`, named after the source
file without its `.rs`. An entry states the block's preconditions, the invariants that make
them hold, and a proof on paper. A fresh agent that did not write the code reviews each
entry adversarially. Changing the block means updating the entry and having it reviewed
again.

### Performance

Performance claims rest on three kinds of measurement.

- **Roofline microbenchmarks** compare each fast path with floor allocators (the cheapest
  code that could serve the workload), with variants that differ from the real path in one
  detail, and with other allocators. They report cycles and instructions from hardware
  counters, which do not depend on clock frequency.
- **Workloads**: multithreaded Rust servers and pipelines in which objects cross threads,
  compiler and data-processing runs, and established allocator benchmarks through the
  preload library. Rust workloads are measured with asmalloc as the global allocator,
  because preloading discards Rust's sized frees.
- **Memory**: resident and huge-page-backed memory over time, fragmentation as resident
  bytes over live bytes, page faults, and how quickly memory returns after a peak.

Comparisons interleave the allocators, pin threads, repeat across process launches, and
include a run of one allocator against itself to expose a broken setup.

The merge gate is deliberately small: formatting, lints, unit tests and the quick proofs.
Long proofs and benchmarks run on demand.
