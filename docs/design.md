# cralloc design

## Summary

cralloc is a memory allocator for multithreaded Rust programs on modern Linux. It serves Rust's
`GlobalAlloc` and `Allocator` traits and the C `malloc` family from one heap, so it works as a crate
dependency or as an `LD_PRELOAD` library.

Three ideas define it.

1. **Measure the floor, then prove the code that reaches it.** The instructions that run on every
   allocation and free are written in assembly. Each sequence is the fastest of several candidates
   measured on real hardware. It is generated from Lean and proved correct, even when the OS
   interrupts it at any instruction.
2. **Per-CPU caches with no atomics and no locks.** The common path pops or pushes a pointer in a
   cache that belongs to the CPU the thread is running on, inside a Linux restartable sequence, and
   commits with a single store. The slow paths are lock-free: every change to shared state is a
   single instruction.
3. **Metadata never lives in freed memory.** A small object's size class comes from its `Layout` in
   Rust and from its address in C; a large object's always comes from its address. Free memory is
   tracked in bitmaps stored elsewhere. Freeing a small object writes only to the caller's own
   cache, so it touches neither the object nor shared metadata, and it costs the same on any thread.

The allocator has three tiers. Each is reached only when the one above runs dry or fills up, and
each handles objects in larger batches than the one above.

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

This document says what the allocator does and why. What we measured is in `docs/research/`,
starting from its `README.md`.

## Background

### What Rust gives an allocator

Rust passes the `Layout` (size and alignment) to `dealloc`, and both the old and new sizes to
`realloc`. C passes only a pointer. With the layout in hand the allocator can compute the size class
on free without looking anything up, and when the layout is a compile-time constant, as it is for
`Box<T>` and most collection nodes, so is the size class.

Rust reaches the global allocator through four symbols (`__rust_alloc` and friends) defined in the
crate that declares `#[global_allocator]`. Code in that crate gets the allocator's fast path
inlined. Code in other crates calls the symbols across a crate boundary, and without link-time
optimization (LTO) also calls an empty marker function on every allocation, which costs more than
the fast path itself. A library cannot turn LTO on for its users; it can only recommend it.

The `Allocator` trait, stable from Rust 1.100, returns the usable length of each block and lets
`deallocate` be called with any size from the requested length to the returned one, so the
size-class function must give the same class for every size in that interval.

### What existing allocators do

- **mimalloc** gives each thread its own pages and keeps free objects in linked lists threaded
  through the objects themselves. Its single-threaded fast path is excellent, and all maintenance
  hangs off one fast-path branch, so work is paced by allocation with no background thread. A free
  from another thread costs an atomic instruction and a cache line shared with the owning thread,
  per object.
- **tcmalloc** caches pointers in per-CPU arrays under restartable sequences and moves them to a
  shared tier in batches. A free never touches the object and looks the same on any thread. Its
  per-CPU mode registers its own restartable-sequence area, which glibc 2.35 and later already own,
  and it caches a pointer in bytes it expects the OS to overwrite on every reschedule, which current
  Linux no longer does.
- **jemalloc** uses per-thread caches of pointer arrays, sized deallocation, and batched flushes to
  shared bins.

Work-stealing runtimes free on a different thread from the one that allocated as a matter of course,
and that is the case the incumbents serve worst: when objects cross threads, mimalloc takes well
over jemalloc's time, and both are several times slower than the cheapest code that can do the same
job (`docs/research/roofline.md`, "The floor and where cralloc stands";
`docs/research/workloads.md`, "The kernel suite").

### What Linux provides

A **restartable sequence** (rseq) is a short run of instructions that the OS promises to restart
from a known point if the thread is preempted, migrated or sent a signal before the run finishes.
glibc registers an rseq area for every thread and publishes its location (`__rseq_offset`,
`__rseq_size`).

The OS also writes the number of the CPU the thread is running on into that area, and changes it
only at an event that also restarts the thread's sequence. A thread with no registration reads a
negative number there.

Together these give mutual exclusion for free. If a sequence reads its CPU number, works on data
that belongs to that CPU, and makes its only visible change with its last instruction, then no other
thread running the same kind of sequence can observe or disturb it: another thread can run on that
CPU only after this one has been stopped, and a stopped sequence is restarted, not resumed.
`docs/research/linux-rseq-contract.md` checks these claims against the kernel and glibc sources.

The area also carries a concurrency ID, a compact numbering of a process's running threads. cralloc
indexes by CPU number instead: it is in every kernel that has rseq, a thread with no registration
can be told from any other with no extra instructions, and current kernels can briefly show a thread
a garbage concurrency ID after a failed `execve`.

### What the hardware says

We measured candidate fast paths and the primitives around them on the reference machine, an AMD Zen
5 (`docs/research/roofline.md`, `docs/research/hardware.md`). Four conclusions shaped the design.

- **The fast path is nearly free.** An inlined allocate and free take a few cycles together. One
  uncontended atomic instruction costs as much as several such pairs, a cache line moved between
  cores many more, and one page fault several hundred. The design effort belongs in not faulting,
  not moving cache lines between cores, and not executing an atomic instruction per object.
- **A per-CPU cache is worth its price.** It costs a few cycles more than a thread-local one when
  inlined and nothing when called, and buys caches bounded by the CPU count, no per-thread state to
  create or tear down, and frees that are local on every thread.
- **Inlining matters more than any other fast-path choice,** and it needs LTO.
- **The last cycle or two depend on what a compiler does not control.** Narrow loads and absolute
  addressing lose zero-delay store forwarding. The processor delivers a straight-line run of up to
  12 micro-ops at full speed and about half speed beyond that, and a routine that leaves only its
  last few instructions in a second 64-byte line pays a cycle. Assembly lets us choose the
  instructions, count the micro-ops, and for out-of-line code fix the size and alignment too.

## Goals

The aim is to dominate the incumbents: faster than each of them, with the same memory or less.

1. **Speed.** Each fast path within a cycle of the cheapest code that could serve the same workload.
   Faster than mimalloc, tcmalloc and jemalloc on multithreaded Rust workloads, especially those
   that free on a different thread from the one that allocated, and on par with mimalloc on
   single-threaded ones.
2. **Memory.** No more resident memory than the leanest incumbent on the same workload. Memory the
   program has stopped using goes back to the OS within seconds, and what is parked in caches is
   bounded by fixed amounts for each CPU and each class. Memory is judged where it costs something:
   on heaps of hundreds of megabytes to gigabytes, as a proportion of the heap. A fixed cost of tens
   of megabytes is spent freely when it buys speed or fewer system calls on such heaps, and the peak
   of a heap of a few megabytes is not something we tune.
3. **One heap, three interfaces.** `GlobalAlloc`, `Allocator` and the C ABI share one heap, and C
   code can free what Rust allocated.
4. **Verified.** The assembly is proved against a machine model. Size-class and layout arithmetic is
   machine-checked for all inputs. Every `unsafe` block has a machine-checked proof or a reviewed
   written one.
5. **Robust by construction.** No locks, so `fork` cannot deadlock the child and a signal handler
   may allocate. No per-thread state, so thread exit needs no code. An all-zero heap is a valid
   heap, so the fast path has no initialization check.
6. **Pure Rust plus proved assembly.** No C. The allocator is `no_std` and never allocates, panics,
   unwinds or prints while serving a request.
7. **One platform, done fully.** cralloc is developed, tested and measured on x86-64 with Linux 7.2,
   Rust 1.99, and glibc 2.43, linked dynamically or statically, or musl. A thread without a
   registered rseq area, as under valgrind, still works, slowly. cralloc never hands a request to
   another allocator.

## Non-Goals

- **Compaction or meshing.** Fragmentation is handled by size-class segregation and reuse policy.
- **Speed without rseq.** That mode exists for correctness under tools such as valgrind. It reuses
  the path large objects take and is not tuned.
- **Other architectures and operating systems.** aarch64 comes after x86-64 is complete; its
  measurements and proofs must be redone.
- **A musl shared library.** On musl the allocator supplies its own rseq area, which it can do in an
  executable. Statically linked musl executables are tested; dynamically linked ones are expected to
  work and are not tested.
- **Hardened-allocator features.** No guard pages, quarantine or randomization. Two protections fall
  out of the design anyway: there is no metadata in freed memory to corrupt, and a double free is
  detected when the object returns to the back end.
- **NUMA awareness.**
- **Heap profiling.**
- **Allocation in a `vfork` or `CLONE_VM` child.** Such a child shares the parent's memory but has
  no rseq registration of its own, so the fast path is unprotected there.
- **More than one instance per process.** Two copies of cralloc, for example the preload library and
  a binary that links the crate, have separate heaps and must not free each other's pointers.

## Implementation

### Vocabulary

- **Class**: a size class. Small classes are cached in magazines; large classes are not.
- **Region**: 4 GiB of address space that holds objects of exactly one class.
- **Magazine**: a small aligned array of pointers to free objects of one class.
- **Slab**: the per-CPU block of state that names the magazine loaded for each small class.
- **Depot**: stacks of full and empty magazines, one set for each group of CPUs that share a
  last-level cache.
- **Routine**: one of the assembly sequences generated from Lean.
- **Twin**: a Rust function with the same specification as a routine, used where assembly cannot
  run.
- **Carve**: take never-used objects from the end of a region.
- **Launder**: return the objects in a magazine to their regions' bitmaps.
- **Purge**: return the physical memory behind free objects to the OS.
- **Heartbeat**: the bounded maintenance done each time a slow path runs.

### Address space

At first use cralloc reserves 16 TiB of address space, inaccessible and uncommitted: about 3940
regions of 4 GiB for objects, and the rest for metadata and magazines. If the process cannot have
that much, as under an address-space limit or valgrind, the region size is halved until the
reservation fits, so the number of regions stays the same. The floor is a 2 MiB region and a
reservation of about 8 GiB; a process allowed less than that cannot start the allocator, and every
allocation fails.

A region is assigned to a class on demand and holds only objects of that class, at `region_base +
origin + index * class_size`. A small class's origin is a few whole pages, different for each class,
so that the first pages of the classes in use do not all share the low page-number bits that index
the TLB's sets; the pages below it are never committed. A large class's origin is zero. A 4096-byte
table records the class of each region.

Every pointer therefore has two properties that hold without per-object metadata:

- its class is `class_of[(ptr - heap_base) >> region_shift]`, one load from a table that fits in the
  L1 cache;
- its index within the region is `(ptr - region_base - origin) / class_size`.

The Rust entry points do not need even that for small objects: they compute the class from the
`Layout`. The table serves the C entry points and large objects. The scheme also removes external
fragmentation between classes, because any free slot in a region fits any request for its class. The
price is that objects of different classes never share a page: a program that uses many classes
lightly has one partly used page per class, and each hot class needs its own TLB entries. The
huge-page policy is built around this.

A class takes a region in two steps: it writes its tag in the table, and then names the region in
the word that says where the class is carving. Of two threads racing, the loser takes its tag back,
so a tag is final only once the region is named. A caller holding a pointer into the region knows it
is; the purge and the other walks do not, so they read a second byte for each region, written once,
after the naming.

A region becomes writable in steps, each a single `mprotect` that about doubles the writable prefix,
up to 64 MiB. Each call holds the kernel's lock on the address space, so the number of calls must
grow with the logarithm of a region's use. Under strict overcommit the commit charge is the writable
prefix, at most about twice what a region has used.

The rest of the reservation holds each region's bitmaps, at an address computed from the region
number, and an array of magazines. A small class keeps its watermark and other header words in
static state, so a small process pays no pages for bitmaps it never uses. Memory that holds
application objects never holds allocator state.

### Size classes and alignment

A request is first rounded up to a multiple of its alignment. Classes are every multiple of 8 bytes
up to 64, then four per doubling. The class of a request is the smallest class at least as large as
the rounded size.

Alignment needs no further work. Regions are aligned to their size, a class's origin is a multiple
of the largest power of two that divides its class size, and objects sit at multiples of the class
size from there, so an object is aligned to that power of two. With four classes per doubling, the
class chosen for a size that is a multiple of `A` is itself a multiple of `A`. Every alignment up to
the region size is satisfied with no over-allocation and no interior pointers. This property, and
the `Allocator` requirement that every size from the request to the class size maps to the same
class, are proved for all inputs with Kani.

The C entry points apply C's rule: a block of 16 bytes or more is 16-byte aligned. So `malloc(24)`
returns a 32-byte block where Rust's `Layout` of size 24 and alignment 8 gets a 24-byte one.

Classes up to 32 KiB are **small**: they are cached in magazines, and Rust computes their class from
the layout. Everything above is **large**: uncached, with the class always read from the address,
because a large object may sit in a slot bigger than it asked for.

### Front end: magazines under rseq

Each CPU has a slab, and the slabs are a static array with an entry for every CPU number the OS can
produce (8192 on x86-64). The address of a slab word is the CPU number shifted and added to the
address of the class's word in slab 0, a link-time constant, with no pointer to load. Untouched
slabs cost no memory. A routine compares the CPU number with the array length and takes the slow
path if it is out of range. That one compare keeps a thread with no rseq registration, which reads a
negative number, away from every slab, and it means no value the OS writes there can index outside
the slabs. A wrong value in range would put two threads on one slab, so that the number is right is
trusted (see "What is trusted").

A slab's hot part gives each small class a 32-byte chunk: the class's 8-byte word at bytes 24 to 31,
and at bytes 0 to 23 words that only slow paths use. The position guards against one hazard. A load
that shares a page offset with a store still in flight can be held until the store completes, and
the hot word is the last thing a call stores, so nothing a fast path loads on every call may share a
page offset with a hot word. The crate places its own tables clear of them, and a test and the build
checks flag a binary whose copy of the C library's rseq offset falls on one, since the linker
decides that placement.

A **magazine** is a block of 8-byte slots, a power of two in size and aligned to its size. Slot 0 is
a link word used by the depot, the last slot holds the fill level while the magazine is not loaded,
and the slots between hold pointers to free objects. Magazines are numbered blocks in one array in
the metadata area; they are created by advancing a counter and are never unmapped. Their size falls
as the class size rises, from 1,024 slots to 4, so no magazine holds more than about 64 KiB of
objects.

The slab word for a class is the address of the next free slot in the loaded magazine. Because a
magazine is aligned to its size, the low bits of that address are the slot number, and the bounds
checks need no other state. For a magazine of `n` slots the mask is `8n - 16`, which covers every
slot-number bit but the lowest:

- allocation takes the slow path when `word & mask` is zero, which means slot 0 or 1: no magazine,
  or an empty one;
- free takes the slow path when `(word + 8) & mask` is zero, which means slot 0 or `n - 1`: no
  magazine, or a full one.

A word of zero fails both tests. A slab that has never been written, which is all zero bytes,
therefore behaves as a slab with no magazines, and nothing needs initializing before the first call.
A **marker** is a word whose low 19 bits are exactly 1, which leaves every slot-number bit clear, so
a marker also fails both tests (see "Idle caches").

The routines rely on one invariant, which the slow paths maintain. For every CPU and small class,
the slab word is zero, a marker, or the address of slot `s` of a magazine of that class that nothing
else refers to, with slots 1 to `s - 1` holding pointers to distinct free objects of that class that
appear nowhere else.

A routine names no class. It is given the address of its class's word in slab 0, and the class's
mask. Allocation has this shape (the exact instructions are chosen by measurement and may change):

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

If the thread is preempted, migrated or signalled anywhere between `start` and `end`, the OS resumes
it at an abort handler that jumps back to the arming store. Nothing before the last instruction
changes shared state, so an aborted run leaves no trace. Free is the mirror image: it stores the
pointer into the free slot, which nothing can observe yet, and commits by storing the incremented
word.

When the class is a compile-time constant, the caller's computation of the two inputs folds to a
`lea` and a `mov`, which is why the routine takes the mask in a register. An executable can read
`__rseq_offset` with one load instead of two, and a cargo feature selects that. A shared library
goes through the indirection shown, with the variable's address in a 16-aligned word of the
routine's own, so that the load stays clear of the hot words.

The routines follow rules that come from the measurements, and the generator refuses to emit one
that breaks them:

- state that the fast path both writes and reads is 64 bits wide and addressed through a base
  register;
- a compare and its branch are adjacent, so they fuse into one micro-op;
- the commit stores a value computed from the word the routine read, never a read-modify-write in
  place, because a drainer may exchange the word between the read and the commit;
- an inlined routine is at most 13 micro-ops, one more than the processor delivers at full speed,
  and inlined allocation costs its caller one more, a fused test for a miss;
- an out-of-line routine starts on a 64-byte boundary, its hot path fits in the 24 micro-ops the
  processor delivers in the time a call and return take regardless, and that hot path is at most 64
  bytes, or between 76 and 128: crossing into a second line is free unless only the last few
  instructions do, and a third line costs a cycle;
- the slow path is out of line.

The pop and the push are used in two settings. For Rust they are `asm!` blocks in `#[inline]`
functions, which become part of `__rust_alloc` in the user's binary and, with LTO, of its callers.
For C they are the last steps of the out-of-line entry routines for `malloc`, `calloc`, `free` and
the sized free, exported as the C symbols themselves, which first turn a size or a pointer into the
word's address through a table. The table of masks is zero until the first slow path writes it, and
a zero mask fails both tests, so it needs no initialization check either.

### Slow path: exchanging magazines

When the fast path fails, the slow path replaces the loaded magazine instead of refilling it. For an
allocation it:

1. obtains a full magazine: from the CPU's spares, else from the depot, else by filling an empty one
   from the back end;
2. installs it with a routine that stores the new word only if the slab word is still what the slow
   path read there: zero, an empty magazine, or a marker left by another process generation. The
   routine returns the old word;
3. retires the old magazine, now private to this thread, to the CPU's spares or the depot, and
   retries the allocation.

If the install fails because another thread got there first, or because this thread moved to a CPU
that already has a magazine, the slow path puts its magazine back and retries. If it fails because
the word is a live marker, or because the thread has no valid CPU number, the slow path serves the
request from the magazine it is holding and sends that magazine to the depot, so it never waits. The
free side is symmetric with an empty magazine.

Every step is one commit. Between steps all shared state is valid and whatever the thread holds is
private to it. This is what makes the slow path safe to interrupt with a signal handler that
allocates, and safe to abandon at any instruction in a forked child.

Each CPU keeps spare full and empty magazines per class, in stacks whose heads live in the cold part
of the slab and are updated by single-store routines. A thread that oscillates around a magazine
boundary swaps with a spare and touches no shared state. The swap is the commonest slow path, so it
carries nothing it does not need: it runs no maintenance of its own. A magazine the spares have no
room for goes to the depot, and a trip to the depot moves one magazine.

A swap installs a magazine at one of its ends, full for an allocation and empty for a free. Work
that swings both ways from where it started then crosses the far end of the new magazine, and so
does every repetition after it: two swaps each time, for as long as the work is steady
(`docs/research/magazines.md`, "The magazine-end lock"). So two reversals in a row are taken as
evidence of such a lock, and the slow path then leaves the loaded magazine half full. A swing of
less than half a magazine never leaves the loaded magazine again. A flow never reverses, and a swing
longer than a magazine reverses once at each turn, not twice, so neither pays for the move.

A swing the half cannot hold would be centred at every turn without ever fitting, so the rule backs
off. A centred swap has failed if another swap of that class on that CPU follows it within an
interval. Each failure in a row doubles the number of the swaps after it that stay plain, up to a
bound, and only an interval without a swap clears the count.

### Depot

The depot holds, per class, a stack of non-empty magazines for each cache domain (the CPUs that
share a last-level cache) and one more shared by all of them, and per magazine size the same for
empty magazines. A slow path that retires a magazine pushes it on its domain's stack while that
holds less than a cap, else on the shared stack. One that needs a magazine pops its domain's stack,
else the shared one, else fills or makes one. The domains are read once from sysfs, on the process's
first slow path, so that a program which has since entered a sandbox is not caught opening a file;
where they cannot be read there is one domain.

A stack head is one 64-bit word: a 32-bit magazine number and a 32-bit version counter, updated by
compare-and-swap. Magazine memory is never unmapped, so reading a stale link is harmless, and the
counter means a stale head is accepted only if exactly 2^32 pushes and pops intervene during one
attempt.

Exchanging through the depot is how objects move between CPUs. A thread that only frees fills
magazines and pushes them; a thread that only allocates pops them. A magazine exchanged costs two
atomic instructions and no work per object, however frees and allocations are spread across threads.

Magazines stay in the cache domain that freed them, because an object's cache lines live where it
was last used, and a magazine handed to another domain brings objects whose every first touch is a
miss. A domain in balance keeps its magazines. A flow of objects from one domain to another fills
the freeing domain's stack to its cap once and then passes through the shared stack, so objects
still cross domains without touching the bitmaps when the program really moves them across. No slow
path reads another domain's stack, so its work does not grow with the number of domains.

### Back end: regions and bitmaps

Each region has a free bitmap with one bit per object, and above it a tree of summary bitmaps, 64 to
1, in which a set bit means "something below may be free". A region also has a watermark: objects at
or above it have never been allocated, and their memory has never been touched.

- **Filling a magazine** searches the class's regions in address order. In each, it walks the
  summary tree to the lowest bitmap word with free objects and claims bits from it with one
  compare-and-swap. The objects of one word are adjacent in memory, so a refilled magazine is
  spatially dense. A fill that meets another fill at a word moves on past that group of 64 words, so
  fills of one class on different CPUs part after a few collisions. A fill claims from a bounded
  number of words and then hands out what it has, so no call pays for more than that many claims. If
  no region of the class has a free object, which only a search from the top may conclude, the fill
  carves from the last region's watermark with one atomic add, and when that region is exhausted it
  assigns a new one to the class.
- **Laundering a magazine** groups its pointers by bitmap word and sets each word's bits with one
  atomic OR. If a bit was already set, the object was freed twice and the process aborts.
- **Summary bits are hints.** They may be set when nothing below is free, and they never stay clear
  when something is. A free sets the word's bits and then sets every summary bit above it that it
  finds clear. Whoever empties a word clears its summary bit and then reads the word again, and sets
  the bit back if the word is no longer empty. A free does not assume that the first free into an
  empty word set the summary bit, because that thread may have vanished in a fork before it could.

The order of preference in a fill is the fragmentation policy: reuse free objects before carving
fresh memory, and take the lowest address first. Live data packs toward the start of each region and
the high end drains. It also makes the highest slot a fill reaches a measure of how much of the
region the program needed, which the purge relies on.

A filled magazine hands its objects out lowest address first, so that the order within a magazine is
the order across magazines. A program that builds a structure and then walks it in the order it made
it reads memory in one direction, and hardware prefetchers follow it.

One fill is the exception. Objects of one size allocated one after another are often siblings: one
for each worker thread, made in a loop and then written by different threads for the life of the
process. Back to back in a class smaller than a cache line they share lines, and every write by one
worker stalls its neighbour (`docs/research/program-effects.md`, "False sharing"). So in classes
smaller than a line, the fill that opens a run, one whose region shows no claim earlier in the
interval, hands out its first objects a stride of slots apart, far enough that no two share a line,
and then the ones between. No slot is skipped, and a fill that continues a run is dense, so a
program building a structure has its stream back after the first magazine.

### Heartbeat

Slow paths do the allocator's maintenance, a bounded amount at a time. It is paced by what moves,
not by calls: a slow path adds the bytes it moved between a magazine and anything beyond it to a
count in its CPU's slab, and runs the maintenance, a heartbeat, when the count passes a threshold.
So a swap with a spare does no maintenance of its own, and the rate does not change when a class's
magazines or fills change size. A large allocation or free counts as a slow path, and beats on one
call in sixteen. This is mimalloc's pacing applied to a per-CPU design.

A heartbeat does one step of purging, one look at a class of its own slab, one look for an idle
cache, and laundering within a budget of one of the largest magazines. A rule that finds work and no
room waits for a later heartbeat. One launder is not paced: a magazine that a call's own exchange
pushed past a shared depot stack's target is laundered on that call, so that the stack cannot gain
on what arrives.

Anything that should happen at a rate in time reads the CPU's cycle counter, which costs no system
call, and acts once the counter has advanced by 2^32 ticks. "An interval" means that: about a second
where the counter runs near 4 GHz, and between 0.7 and 4.3 seconds on any plausible machine. Every
time given below stretches the same way.

Intervals are counted, not computed from the clock. The count steps by one when a heartbeat finds
that an interval has passed since the last step, however long ago that was, so a process that was
silent for a minute has aged everything it holds by one interval. The evidence the rules act on is
that something went unused while the program was running, and a silence is not that. Counted by the
clock alone, the first requests after a silence would find every cache marked idle and pay to tear
down what they are about to use (`docs/research/servers.md`, "The first requests after a silence").

Nothing runs on the fast path, so a thread served entirely by its loaded magazines never runs a
heartbeat. Other threads cover for it: any slow path maintains the shared structures, and the idle
check drains a slab that has had no slow path for an interval, which sends that CPU to its slow
path. Only a process in which every thread stays inside its magazines has no heartbeat at all, and
it keeps what it held when it went quiet. That is where steady work on one thread ends up: a swing
smaller than a magazine settles into a loaded magazine that holds exactly one swing, and never
leaves it (`docs/research/memory.md`, "After a peak, and in a silence"). Nothing on the freeing path
can stand in for the clock, because a free cannot tell memory that is idle from memory about to be
reused. Memory that should go back during a silence needs something that runs during it.

#### The background thread

By default a thread starts itself once two things are true together: the process holds more free
resident memory than the floor (see "Sizing the caches"), and the program has created a thread of
its own. A small program's thread never runs, because it holds nothing worth returning. A program
with one thread never gets one, because it may depend on staying that way; once quiet it keeps its
free memory until it calls `maintain` or `collect`, as it would under glibc's allocator, and we
accept that as a limit. The evidence of a second thread is the C library's own record that one was
created. Where there is no such record, and in a fork child, it is a free by a thread other than the
first. The condition is a level, looked at when free memory rises over the floor and once an
interval while it stays there, so a moment at which the thread could not be created only delays it.

The thread wakes every half second and calls `maintain`, which runs one round an interval and does
for the whole heap what the heartbeats of an interval would have done: the idle check on every slab,
each depot stack's shedding, and the clocked purge, in bounded steps. A slab it drains has its
magazines laundered, not parked in the depot, where a CPU busy on fast paths would take them
straight back.

The thread parks when its rounds have found no work for as many intervals in a row as the longest of
its rules takes to act, and no slow path has run anywhere in that time. It then waits on a word with
no timeout, so a silent process is not woken by its allocator at all. How much memory the process
still holds is not part of this: a fragmented heap can hold more than the floor that no round can
return. By then its rounds have drained every slab, so the first call after the silence takes a slow
path, which finds the word set, clears it and wakes the thread. That costs each slow path one load,
and the fast path nothing. Where slabs cannot be drained, nothing would send a resuming program to a
slow path, so there the thread keeps to its timed sleep.

While the thread runs, returning memory is its work and not a slow path's, because a request that
returns memory delays every request queued behind it on that worker
(`docs/research/large-blocks.md`, "A doubling's tail is the purge that falls on it"). A slow path
that finds such work due records it and wakes the thread if it sleeps. What a request still returns
is what its own call decides: the trim at the claim of a large slot, the tail of a block that
shrinks in place, the pieces behind a block that moves, and a freed slot that was under huge-page
advice or left unwritten. Slow paths keep the clocked work where there is no thread, which is why
every step stays bounded. The bound on retained memory does not rest on the scheduler either: when
growth has gone unpaid past a limit the carves and heartbeats that find it pay a step each, and a
pass nobody has stepped for an interval is the slow paths' again. The thread does not give up the
CPU between its steps, or it falls behind on a busy machine (`docs/research/memory.md`, "Returning
memory off request paths").

The thread is an ordinary thread created through the C library, so that what the library does to
every thread, a change of credentials above all, does not pass it by. Creating one is not safe in a
signal handler or inside the library's own locked sections. So in a Rust program it is created on a
heartbeat of a call that came through the Rust interface, which the C library does not use, and only
if the calling thread has no signal blocked and is not on a signal stack. Two cases that test cannot
see are limits we accept: a handler installed with `SA_NODEFER` and no stack of its own, and a
callback of `dl_iterate_phdr`, which runs under a loader lock. The preload library, whose caller may
always be the C library, exports `pthread_create`, creates the thread with the program's first, and
leaves it dormant until the heap is over the floor.

A cargo feature or an environment variable turns the thread off, and a program can start it itself.
It takes the C library's default stack (`docs/research/portability.md`, "The background thread's
stack"). If it cannot be created the allocator carries on without it. A process under a system-call
filter that kills on thread creation, which no allocator can detect, must turn it off. A fork child
starts its own on the same conditions.

A program with a timer of its own can call `maintain` from it instead, and then does not get the
thread by default. For an async runtime that is a task that wakes every half second, and we ship one
for tokio, in a crate of its own because the allocator has no dependencies. `maintain` limits its
own rate, so it is harmless to call more often, but a hook that runs when a worker parks does not
replace the timer: a worker parks once at the start of a silence.

### Sizing the caches

Every buffer in the allocator stands between a cheap tier and a dearer one. Each has a bound, and
the bounds are constants: one for each class, each CPU or each cache domain. They do not scale with
the heap, and but for one rule, stock, they do not adapt to the program. So what a process parks in
its caches depends on how many CPUs it runs on and how many classes it uses, and not on how much it
has live. The fast path pays nothing for any of it. A design in which every size follows the program
was studied and not built (`docs/research/magazines.md`, "Sizing on evidence").

- **A magazine** holds at most 64 KiB of objects, in 4 to 1,024 slots. That one size is both how far
  a thread can swing without leaving the fast path, and how much moves in one exchange.
- **A CPU's spares.** A megabyte of objects for each CPU is split evenly among the small classes,
  and a class keeps as many full spares as fit in its share, four at most. A full magazine of most
  classes is bigger than a share, so only the smallest classes keep a full spare this way. An empty
  spare holds no objects, and every class keeps up to four.
- **Stock** lets the classes a CPU uses take the shares of those it does not, as below.
- **The depot.** A class's stack in a cache domain takes magazines while it holds less than 32 KiB
  of objects, and the shared stack is laundered down to 96 KiB: 128 KiB a class in a process on one
  domain, and at least one magazine in each stack.
- **Free memory that is still resident has a floor** of 32 MiB. Under it the clocked rules return
  nothing, and the background thread has no reason to start. **Growth pays** above a cap of 64 MiB
  of retained memory (see "Returning memory and huge pages").
- **The bounds on work, and the interval, are constants.** How much one call may launder, purge or
  fill bounds latency.

Stock is spare magazines a class keeps on a CPU beyond its fixed limit. A request handler that
allocates many objects and then frees them all moves more magazines in a burst than the fixed spares
hold, and against a depot stack other CPUs are using, each trip costs a cache line handed between
cores and compare-and-swaps that fail. So a CPU keeps what it has shown that it sends away and takes
back, when the sending is dear: a slow path that sends a magazine to the depot because its spare
stack is full marks the class, and when that CPU next takes such a magazine back, if a
compare-and-swap failed on either trip, the class may keep one more spare there. All stock on a CPU
stays within what the megabyte leaves after the fixed spares. The words that record this are hints:
a lost one moves a magazine to the depot or keeps it one exchange longer.

What a buffer holds shrinks on the clock, by the rules below.

### Returning memory and huge pages

A huge-page fault delivers 512 pages for about a fifth of the per-page price of small faults, and a
page returned too early is faulted straight back. Against that, memory the program has stopped using
must go back to the OS within seconds. Memory the program is not using sits in one of the places
below, and each has a rule that returns it (`docs/research/memory.md` and
`docs/research/large-blocks.md` have the evidence).

| What holds the memory | What returns it | When |
|---|---|---|
| Free slots on pages with nothing live, above what fills reached in the last two intervals | The clocked purge | Once an interval, down to the floor |
| Free pages of a huge page above that frontier, when a quarter of it or more is free | The clocked purge splits the huge page and returns them | When the pass reaches it |
| Free slots of large classes, above the floor | Heartbeats, or `maintain` in a silence | After a short hold without a free that left them resident |
| An occupied large slot's pages past its block and its last tenant's | The claim of the slot | At the claim, one bounded step |
| Any retained memory, while over the cap and the heap grows elsewhere | The carve that grows the heap, and the heartbeats after it | A bounded step each |
| A slot a large block outgrew, beyond a bound on such slots | The move | Behind the copy, a piece at a time |
| The tail of a large block that shrinks in place | The resize | At once |
| Objects in a magazine that stays loaded | A slow path of the same CPU launders it | After an interval loaded |
| Objects in a CPU's full spares | That CPU's sweep launders one and sends the rest to the depot | Once an interval |
| Stock a class no longer needs on a CPU | The sweep takes it off the class's credit and returns it | A magazine a heartbeat |
| Objects in a depot stack | Heartbeats launder the shared stack; every stack sheds what it did not need | While over target, and for the depth an interval did not use |
| Objects in steady circulation | The free that retires a full magazine launders it instead | One retirement in 32, up to one in 4 on a fragmented heap |
| The slab of a CPU the process no longer runs on | The idle drain | After an interval with no slow path there |
| Free pages of a huge page that fills still reach, or that is less than a quarter free | Nothing: the huge page is worth more whole | Until it is wholly free, or fills stop reaching it |
| Free slots on pages that also hold live objects | Nothing: that is fragmentation, and it stays counted | Never |
| All of the above, without waiting for the clock | `collect`, and `malloc_trim` in C | When the program calls it |

**Purge late, and only what fills did not reach.** Purging claims the bits of every object that
overlaps a range of pages, exactly as an allocation would. If all of them were free it returns the
pages with `madvise(MADV_DONTNEED)`, and either way it then releases the bits. Nothing else can
allocate the range while it is claimed, so no lock is needed. What to purge is decided on evidence
of reuse. Fills take the lowest free slots of a region, so the highest slot a region's claims
reached during an interval shows that everything below it was needed. Each region keeps that
frontier for the interval under way and the one before, in one word that also names its interval, so
nothing has to age the words. Once an interval a pass carries on round the heap from where the last
one stopped, returning free pages above the frontiers in bounded steps. Its goal is everything the
last lap found it could return, less the floor: the frontiers already keep that to what two
intervals did not reuse (`docs/research/memory.md`, "How much a clocked pass returns"). While the
program runs it never takes pages below a frontier, since those are the pages the next fills take.
Only an interval in which some slow path ran counts toward a frontier's age, because a silence is
not evidence that the program has stopped needing what it last used. In a silence a pass goes below
the frontiers only while more than the floor could still be returned, each region from its top down.
A frontier is a hint: a wrong one costs a purge too many or too few, never a page in use, because
the purge is a claim.

**Purge in runs, and let short runs wait.** Each call to return memory costs a system call and a
round of interrupts to the other CPUs, whatever its size, so a pass returns adjacent free pages
together, up to a bound that also limits how long a claim is held. A single free page among live
ones is the dearest memory to return and the likeliest to be refilled. So most passes take only runs
of at least a few pages, and one pass in every few takes a run of any length. Growth over the cap,
and `collect`, take any run.

**A step on a request's slow path is bounded.** Every step a slow path takes, of the clocked pass,
of the hold or of a growth payment, returns at most a fixed amount of memory, since a millisecond
spent there once a second is a server's tail. A free large slot bigger than that goes back in
pieces, from its top down, in whole huge pages where its class is that large. It counts as zero only
once every page of it has gone. `collect` and `malloc_trim` are the program's own calls and are not
bounded.

**Free large slots do not wait for the frontier.** A large class's region serves one size, so a
program that frees blocks of many large sizes between phases holds them all for the two intervals
the frontier rule waits (`docs/research/large-blocks.md`, "Free slots: a hold, and why no hold
serves everyone"). So they follow a shorter clock: a hold of some tens of steps of a fine count,
which steps every couple of milliseconds that a heartbeat observes. A large free that leaves its
slot resident stamps both its region and the slot. Once a region has gone a hold without such a
free, heartbeats return its free slots, a bounded amount each, while the free large memory still
resident exceeds the floor. A region whose tenants keep churning is walked at most once a hold and
returns the slots that have themselves been free that long. The hold is a constant: a shorter one
returns memory about to be reused, a longer one keeps the peak, and neither a hold learned per
region nor a long hold under which growth pays did better on both.

**Under a floor, the clock returns nothing.** The floor is measured against what a pass could
return: resident pages with nothing live on them, and while the program runs only those above the
frontiers. While that totals no more than the floor the clocked pass returns nothing, and above it a
pass stops at the floor. It is not measured against all the free memory the heap retains, because a
large heap holds many times the floor in free slots beside live neighbours that nothing can return.
`collect` is not floored.

**Retained memory is counted, and growth pays for it.** The back end keeps an estimate of the free
memory it holds that is still resident, small and large classes together. A free never returns
memory because the count is high: a level cannot tell idle memory from memory about to be reused,
and only the clock can. What a level is good for is a bound. The address space is divided by class,
so a program that frees a great deal of one size and then allocates as much of another would hold
both. So while the count is over the cap, memory carved past a watermark is matched by an equal
purge of retained memory, from wherever the purge walk stands and whether or not fills still reach
it. The carve returns one bounded step and the heartbeats that follow pay the rest, and a debt whose
walk finds nothing to return is dropped. This is the one place a frontier is ignored. Taking back
pages a purge returned is reuse, not growth, and is not charged. A large heap can sit over the cap
for good: its growth always pays, and what it has stopped using goes back by the clock, but the
memory it is reusing is left alone.

**Parked objects do not pin the heap.** An object parked in a magazine holds its page as surely as a
live one, and parked objects are a random sample of wherever the heap has been. Four rules keep them
moving back to the bitmaps, where the next fill takes the lowest free addresses instead.

- A magazine does not stay loaded for more than an interval. A slow path looks at one class of its
  own slab in turn, and launders a magazine that has been loaded that long. A class in use pays one
  refill per interval, and a class that fell out of use on a busy CPU is emptied.
- Full spares do not stay either. Once an interval each CPU sweeps its own slab, a class per
  heartbeat, laundering one full spare and sending the rest to the depot. On a CPU that only frees a
  class, nothing else would ever pop them.
- A depot stack sheds what it did not need: the depth it never fell below during an interval is
  laundered, one magazine per heartbeat. Each periodic look also visits the stacks of one other
  domain whose own CPUs have not looked this interval, so a domain that goes quiet still sheds.
- One retiring full magazine in every few is laundered instead of kept, so objects in steady
  circulation are gradually replaced by the lowest free ones. How few follows evidence: each lap of
  the purge walk measures the small classes' free slots on pages that also hold live objects, as a
  share of what those classes have carved, and from a share of 1 in 64 the rate doubles in steps
  from one retirement in 32 to one in 4. It steps back down as the evidence goes
  (`docs/research/memory.md`, "Refresh on evidence").

**Huge pages only where objects are dense.** A region starts marked against transparent huge pages,
so a lightly used class costs only the 4 KiB pages it touches. Two things count as evidence of dense
use. A class of objects of at most a kilobyte that has carved enough for one huge page to be a small
fraction of it has the rest of its region marked for huge pages. And a large block that grows, by
doubling or less and past the size of a huge page, with the last piece it wrote at least half full,
is being appended to, so the part of its slot up to the size it asked for is marked while it holds
the block. A block that grows in place over a huge page it had only started has that page returned
and its first bytes written back, so that the page faults whole. Small classes above a kilobyte
never get huge pages, because such a block is often touched in part and each part would cost 2 MiB.
A large block allocated at its size in one call gets none either, a limit we accept: a wrong guess
holds many times the block's memory, and telling dense from sparse would put a second writer on a
slot the design gives to its holder alone (`docs/research/large-blocks.md`, "Huge pages only for
blocks that prove dense"; `docs/research/program-effects.md`, "TLB and huge pages").

**Split a huge page only where fills have stopped reaching.** Below a region's frontier, in its
huge-page part, purging works only in whole 2 MiB units. That includes the unit holding the
watermark, whose upper part the fault made resident: a purge first carves the rest of that unit and
frees it. Above the frontier whole units are not enough. When a class's demand falls its short-lived
objects pack into the bottom of the region, and the top is left a few long-lived objects to a huge
page, each holding 2 MiB (`docs/research/memory.md`, "When the size mix wanders, huge pages
ratcheted"). So a step that may return memory treats the huge pages above the frontier as small
pages. One wholly free goes back whole. One with a quarter of its pages or more free and resident is
split, and its free runs are returned. A sample of a few pages, at places that move with every lap,
chooses which huge pages are counted, so a lap over dense huge pages reads little. The purge asks
the kernel to split a huge page before it returns part of it, since pages returned from an unsplit
huge page stay charged to the cgroup until reclaim splits it.

The advice follows what went back. Under the advice the kernel rebuilds any huge page with one page
in use, so when a pass is done with a region it takes the advice off in one call, from the lowest
huge page it returned part of upward and never below the frontier, and not at all where nothing went
back, because a change of advice takes the address-space lock for writing. Each region keeps that
boundary in one word. A claim that reaches above it marks the word, and the background thread, or
else a heartbeat, puts the advice back on, up to the frontier and a few huge pages beyond. Since the
kernel may have rebuilt a huge page, a free page of one that holds anything is taken for resident
under the advice. One thread changes a region's advice at a time, under an owner bit in the word
that another may take over after two intervals, and a kind of call the kernel refused is not made
again before the next interval.

**`collect`**, and `malloc_trim` in C, do all of this at once without waiting for the clock: drain
every slab in use, launder every depot stack, purge what is free, and split whatever they can return
wherever the frontiers stand.

**Locked memory gets nothing back.** Every rule here returns memory with the one call, and the
kernel refuses it on memory under `mlock` or `mlockall`. Free memory then stays resident and
counted, and each rule makes its call, fails and goes on. A freed block larger than a region gives
its regions back only once its memory has been returned, so with memory locked those regions are
never used again. This is a limit we accept.

**Zeroed memory.** Memory at or above the watermark is zero, and so is purged memory. For large
classes a second bitmap records which free objects are still zero, so zeroed allocation skips the
`memset` when it can. Purging sets those bits for objects wholly inside the purged range, before it
releases their free bits. Small classes do not track this, since zeroing a small object costs less
than the bookkeeping.

A zeroed request whose slot is not known to be zero clears it, which makes every page resident
whether or not the program goes on to write there. Returning the pages instead would fix a program
that writes a little of a large zeroed buffer and hurt one that writes all of it, and no size
separates the two, so the free decides from evidence. A zeroed request of 16 MiB or more answered
from a slot known to be zero is marked. When it is freed, the free asks the kernel about one page in
each sixteenth of the block. If at most one is resident the slot goes back to the kernel whole, at
once, and is recorded as zero and not retained; otherwise it is an ordinary free. What the look
finds chooses between two valid frees and nothing else, so it does not have to be right
(`docs/research/large-blocks.md`, "Zeroed blocks: evidence at the free").

### Large objects

Large classes have no magazines. Allocation and free operate on the region bitmap directly, at the
cost of a few atomic instructions, which is small next to the cost of touching the object. An object
larger than a region takes a run of whole regions.

The current size of each large object is recorded out of band, one word per slot, written only when
the object is allocated, resized or freed. A free leaves the size there for the slot's next claim,
and adds when the free happened. C's `realloc` needs it to know how much to copy, and the usable
size must not be the whole slot: a caller that fills what it is told it may use would touch many
times what it asked for.

A large object does not start at the start of its slot. Slots of a class are all aligned alike, so
objects that began there would put their cache lines in the same sets, and a program that walks many
such buffers together would miss the cache on every access. An object whose alignment is at most a
cache line starts a multiple of 64 bytes into its slot, chosen by the slot's index, and is given a
page more room so that any such start fits. Nothing is stored: a block starts at its slot's start or
at that offset, and any other pointer into the slot is not a block.

`realloc` and `grow` succeed in place whenever the new size fits the slot and the caller's next
`dealloc` will still find the right class: for a small object the new size maps to the same class,
and for a large one it is still large. Anything else moves the object.

**Growing.** A large object that has to move because it grew moves into a slot with room to grow in
place, so a buffer that doubles repeatedly is copied a few times in total. A heap with its full
reservation gives sixteen times the block's size where that slot is at most a sixteenth of a region;
otherwise the room is four times where that fits a region, and none past that. Each falls back to
the next when the larger slot cannot be had, and a grow that must hand back zeroed memory takes no
room. The room is given only where a region still holds many such slots, because a region is never
taken back from a class. It costs address space and, under strict overcommit, commit charge, but not
memory: pages nobody wrote are not resident.

**Shrinking.** A large object that shrinks in place returns the whole pages past its new size at
once. One that shrinks to a small fraction of its slot moves instead, so that a slot is not held for
good by a tiny object. The fraction is half of what a block fills when it moves into a slot of that
size with room, so a block resized back and forth across a class boundary settles in one slot.

**Moving.** The copy goes in pieces, and each piece's pages are tested first. Zero pages are left
out and the destination is purged only under them, so a block the program wrote sparsely stays
sparse. Unless the old slot is kept, each piece's pages go back behind the copy, so the two copies
are never both resident. Moving the pages with `mremap` instead would copy nothing, but it splits
the heap's mapping in ways that take several more rules to undo, and saves no memory.

**What growth leaves behind** pulls two ways: a buffer grown and freed again and again should find
its memory resident each time, and many buffers growing at once should not leave a trail of the
blocks they outgrew. So a slot a block has outgrown is kept resident, whole, while the outgrown
slots that nothing has taken again total no more than 64 MiB, and goes back behind the copy beyond
that. This is the one place a free acts on a level, and it may because of what the level counts:
only slots a growing block left behind and nothing has reused (`docs/research/large-blocks.md`,
"What growth leaves behind"). A slot under huge-page advice is not kept, nor is one that carries the
mark of an unwritten zeroed block. A grown block that is freed is an ordinary free, unless it
carries huge-page advice, when its slot goes back whole.

**What a free slot is charged.** A free slot counts, against retained memory, the pages that may be
resident, not its whole size. The record is the slot's clean-page bits, and a clean bit is true: the
page has not been written since it was carved or last purged. So the free, the claim, the keep
decision and the purge count a slot by the same bits and cannot disagree.

**What earlier tenants left.** The pages a larger tenant wrote past a smaller one's block stay
resident inside an occupied slot, where no purge reaches them, and in a slot that tenants of many
sizes keep taking what is resident would be the high-water mark of them all. So the claim of a free
slot returns what lies past both its new block and the last tenant's, from the top down, and one
purge step at most. Only the claimer does this, because nothing but its holder touches an occupied
slot. The price is page faults when a later, larger tenant needs the room back
(`docs/research/large-blocks.md`, "Slack: what earlier tenants left in an occupied slot").

### Interfaces

- **`GlobalAlloc`** for `#[global_allocator]`. `dealloc` of a small object is the free routine and
  nothing else.
- **`Allocator`**, on Rust versions that have it, through a zero-sized handle. All handles share the
  one heap. `allocate` returns the full class size of a small block and the usable size of a large
  one. A zero-size request gets a dangling, aligned pointer and no block.
- **C ABI**: `malloc`, `calloc`, `realloc`, `reallocarray`, `free`, the aligned-allocation family,
  `malloc_usable_size`, `malloc_trim` and the sized frees: every function glibc exports that takes
  or returns a block. `free(p)` reads the class from the address table and joins the same path as a
  Rust free. A pointer outside the heap is a caller's bug and aborts the process. A pointer into the
  middle of a small object is the same bug, but the fast path does not look for it: it is caught
  only if it reaches the bitmaps before it is handed out again. `malloc_usable_size` reports the
  class size of a small object and, for a large one, its recorded size extended to the end of the
  page it ends in. `realloc(p, 0)` frees `p` and returns null, as glibc does.
- **A size from C is a prediction, never trusted.** A sized free may use the size to find the class
  early, but the class from the address is the truth, and a wrong size behaves exactly like a free
  with no size.
- **C++ operators are not exported.** A C++ program's `operator new` and `operator delete` stay its
  C++ runtime's, which calls our `malloc` and `free`; that is correct in every arrangement of
  libraries. Exporting the operators is sound only if we export all of them, and we cannot: a
  throwing `new` that fails must raise `std::bad_alloc`, and an exception cannot unwind through the
  allocator's Rust frames. The price is a call layer for C++ (`docs/research/roofline.md`, "C++ new
  and delete without our operators").
- **`collect`**, a Rust function and what `malloc_trim` calls: return everything that can be
  returned now. `inspect` reports where the heap's memory is, without changing it.
- **`maintain`**: one round of the clocked maintenance for the whole heap (see "Heartbeat").

A macro exports the C names from the crate that invokes it; a cargo feature does the same, and the
`LD_PRELOAD` library always exports them. A feature alone would not do: cargo unifies features
across a workspace, and every test binary would then replace `malloc`.

The allocator's only references to a C library are these: `errno`'s accessor and glibc's
`__rseq_offset`; on musl, `errno`'s accessor and the three pthread key functions that run code at
thread exit; and in the default build, what creating the background thread takes.

The recommended build is `lto = "fat"`. Thin LTO also inlines the fast path into dependency crates,
but it silently stops doing so when the crate that declares `#[global_allocator]` contains a private
`#[used]` static or module-level assembly; declaring the allocator in a small crate of its own
avoids that.

C may free, resize or measure anything Rust allocated. Memory obtained from the C entry points must
be released through them, which Rust's own rules already require: its class may differ from the one
a Rust `Layout` of the same size implies.

### Process lifecycle

- **Start.** The static slabs are zero, so the first call takes the slow path, which reserves
  address space and publishes the heap with one compare-and-swap. Threads may race to do this; the
  losers unmap their reservations. Only a few full reservations fit in the address space at once, so
  a reservation that fails while another thread's is in flight says nothing about how much the
  process may have: the thread looks for the published heap and tries again at the same size. A
  failure with no other reserver in flight, or a bounded number of failed tries, makes it settle for
  smaller regions or report failure.
- **Unloading.** A shared object that contains the allocator must not be unloaded: a thread's rseq
  area goes on naming a sequence inside it, the kernel reads that the next time the thread is
  preempted, and the process dies. The preload library is linked so that it cannot be unloaded, and
  anyone else who builds the allocator into a shared object links it the same way.
- **Without rseq.** A thread with no registration reads a negative CPU number, so every routine
  sends it to the slow path and it never touches a slab. That covers a whole process under valgrind
  or with the C library's registration turned off, and on musl a single thread whose registration a
  sandbox refused. The slow path then serves small objects the way it serves large ones, straight
  from the bitmaps. This is the same code, and it costs tens of cycles per call instead of a few.
- **Under Miri.** Miri runs neither assembly nor system calls, so the allocator's handle does not
  exist there. A program checked with Miri declares its global allocator only when not under Miri.
  Our own tests run the allocator under Miri with the twins and the simulated OS.
- **Threads.** A thread has no allocator state. Starting and exiting need no code, and objects
  cached under a CPU stay with that CPU.
- **Threads on musl.** musl registers no rseq area, so the allocator defines one per thread, in
  thread-local data emitted with the routines. It reads a negative CPU number until the thread's
  first slow path registers it. musl frees a thread's storage before the thread is gone, so a
  pthread key's destructor unregisters the area and marks it retired, and a retired or refused area
  reads a negative number for good. Three limits follow. Making the key takes a lock inside musl, so
  a signal handler that makes the process's first allocator call while its thread is inside musl's
  key functions deadlocks. A thread whose first allocator call comes after the destructor can no
  longer run registers and dies registered, which can take the process with it. And destructors that
  run after ours free on the slow path.
- **Idle caches.** Memory can sit in the slab of a CPU the process no longer runs on. Each slab has
  an activity counter that its slow paths advance, and a heartbeat that finds one unchanged for an
  interval drains that slab. A slab busy on fast paths alone also looks idle, and is drained once
  per interval, which costs it one refill. Every process has a **generation**, a random number; its
  marker is the generation shifted above the low 19 bits, with the lowest bit set.
  1. Claim the slab by writing this generation into its drain flag with a compare-and-swap. If the
     flag already holds this generation, another thread is draining; stop.
  2. Exchange each class's slab word and each spare-stack head with this generation's marker, one
     atomic instruction each, keeping the old values.
  3. Ask the OS to restart every sequence in flight on that CPU (`membarrier`). If the call fails,
     put each old value back with a compare-and-swap against the marker, release the claim and stop.
  4. Read each word again. If it is still the marker, no thread can commit to it, and the drainer
     owns the old value: it stores zero and, if the old value named a magazine, sends it to the
     depot. If it is anything else, a sequence that started before step 2 committed, the old value
     still belongs to that CPU, and the drainer takes nothing and writes nothing.
  5. Release the claim.

The routines on the fast path treat any marker as "no magazine" or "empty stack" and never store
over one, so only a commit from a sequence already in flight can replace a marker, and step 3 ends
all of those. This is the only time one CPU's magazine words are written from another. Where the OS
refuses to register for the `membarrier` command, draining is off.
- **Fork.** The child gets a copy of a heap in which every shared structure is valid at every
  instant. Objects that another thread held privately at the moment of the fork are lost to the
  child: a small, bounded leak. A drain that was under way leaves a claim and markers with no
  drainer. The generation lives on a page that is wiped on fork, and the child's first slow path
  draws a new one, so the child sees the parent's claims and markers as belonging to another
  generation and treats them as absent. That same slow path clears any owner bit left on a region's
  huge-page advice, has each region's next change make the kernel's advice agree with its word
  first, and marks every region a class is carving or has carved from as named for the walks.
- **Signals.** A handler may allocate and free. A signal aborts any sequence in flight, and the slow
  paths hold nothing that a nested call could trip over. The exceptions are where the allocator
  calls the C library, which has locks of its own: the creation of the background thread, and on
  musl the process's first registration.

### Concurrency rules

These hold everywhere, and the proofs and tests lean on them.

1. A slab's magazine words, the hot word and the two spare-stack heads of each class, change only
   through an rseq routine that commits with one store, or through the drain steps under "Idle
   caches". The slab invariant is about these words and no others.
2. Other shared state changes only through single atomic instructions. That includes the rest of a
   slab: its counters, stamps, sweep position and stock words. They are hints, kept with a plain
   load and a plain store by whatever slow path is on that CPU, and each is one whose wrong value
   moves a launder, a drain or a magazine earlier or later and does nothing else.
3. No locks, no waiting, no thread-local state. The rseq area that the OS's interface requires of
   each thread is an exception, and on musl it is ours. The background thread is another: it sleeps
   on a word of its own, and no request ever waits for it. And where the allocator calls the C
   library it is subject to that library's locks.
4. Between any two instructions of any slow path, shared state is valid and anything unreachable
   from it is owned by exactly one thread.
5. All-zero is a valid state for every structure.

### Components

Components depend downward only: the interfaces on the front end and large objects, those on the
depot and back end, everything on size classes and the OS interface. The OS interface is a trait
with a Linux implementation built on raw system calls and a simulated implementation for tests.

The crate's public API is small: the allocator's handle, the statistics it reports, the background
thread's controls, `abort`, and the macro that exports the C names. The components are private. The
test kit, the fuzz targets and the benchmarks reach them through a hidden cargo feature that is not
part of the API.

## Testing and Verification

### What is proved, and how

| Layer | Method | Strength |
|---|---|---|
| Assembly routines | Lean, against a model of the instructions and of rseq | Every input, every interruption point |
| Size classes, alignment, slot masks, bitmap word operations, address and layout arithmetic, packed words, the decisions of the clocked rules | Kani | Every input |
| The twins | Kani, against the routines' specifications | Every state of one small slab, with an interruption before any one step |
| The lock-free protocols as designed: the routines, magazine exchange, drain, depot stacks, summary bits, purge, the assignment of regions, the boundary of huge-page advice, start-up, fork | A TLA+ model of each, in which the OS aborts, moves, fences and forks threads, checked exhaustively with TLC | Every interleaving of the model, within small bounds |
| The Rust that implements those protocols | Schedule explorers in the test code, which run it unchanged under every schedule within a bound | Bounded schedules, on small heaps |
| The allocator as a whole | Model-based tests, Miri, fuzzing, and real programs under the preload library | Testing |
| `unsafe` blocks none of the above reaches | Safety ledger | Written proof, adversarially reviewed |

No proof covers a run of operations on a whole heap. Bounded model checking does not get through the
back end's nested retry loops, so that ground is held by the explorers and the model-based tests.

The property all of this serves: every operation behaves as one atomic step of an abstract allocator
whose state is the set of live blocks. A returned block is aligned, at least as large as requested,
inside the heap, and disjoint from every live block. The allocator never reads or writes the bytes
of a live block except where the API says so, and never touches memory it does not own.

### What is trusted

The proofs assume the following and nothing else about the platform. Each is exercised by tests
where a test can reach it.

- A thread that is preempted, migrated or signalled inside a registered sequence resumes at the
  sequence's abort handler.
- The CPU number in a thread's rseq area is the CPU it is running on, changes only at an event that
  aborts its sequence, and is negative while the thread has no registration. Nothing else is assumed
  about its value.
- `membarrier` with the rseq command returns only after every sequence then in flight on the named
  CPU has completed or been aborted.
- glibc registers a thread's rseq area before the thread can call the allocator. On musl, where the
  area is ours, it reads a negative CPU number until registered, and the OS accepts one registration
  per thread.
- On musl, a signal handler that allocates does not make its thread's first call to the allocator at
  the one instruction boundary between a routine's arming store and its read of the CPU number.
  There the handler's slow path would register the thread under a sequence that began unregistered,
  and that one sequence would run to its commit unprotected.
- Nothing with the privilege to do so places a probe inside a sequence, which would let the thread
  return to it without an abort.
- Of a thread's attempts at a sequence, one is at last left to finish. Only termination rests on
  this.
- Fresh anonymous memory, and memory after `MADV_DONTNEED`, reads as zero.
- The instruction model, the Rust compiler and assembler, and Lean's kernel.

### Assembly

All assembly is generated from Lean; none is written by hand. A routine is a Lean term with a
specification and a proof, and the generator prints it as Rust source that is checked in. A check
fails if the checked-in assembly differs from what Lean generates, if any proof uses an axiom beyond
the standard ones or does not replay through Lean's kernel, or if an out-of-line routine, once
assembled, breaks the size and alignment rules under "Front end". The check is itself tested against
forged proofs.

For each routine we prove four things. Run to completion, it performs its specified step on the
slab. Stopped before any instruction, including the last, it has changed nothing the specification
can see, and the registers the restart needs are intact. It reads and writes only the thread's rseq
area, the slab array and the magazine the slab word names. It clobbers only the registers it
declares.

The proofs assume, at entry and at every retry, the part of the slab invariant a routine can see:
its class's word is zero, a marker, or a slot address of a magazine of the class's size, and that
magazine lies apart from the slabs, the thread's rseq area and the stack. The Rust slow paths keep
the whole invariant. The layout constants the proofs assume are emitted with the routines, and the
Rust code is held to them.

The framework's trusted core is the instruction semantics, the rseq interruption model, the printer,
the meaning of Rust's `asm!` operands, and each routine's stated contract. Changes to these are
reviewed separately from changes to routines.

### Models of the protocols

The protocols on which memory safety rests each have a TLA+ model in which the OS is an actor: it
stops, moves and restarts threads, runs signal handlers, fences, forks, and hands out address space.
TLC checks every interleaving of a few threads over a few magazines and objects, which is as many
actors as the flaws these protocols can have need. A configuration that is expected to fail is kept
as a test: it shows that the model can see a flaw, and that the rule which removes it is needed.
`tla/README.md` lists them.

The other words that more than one thread writes have no model. A flaw in any of them can cost
memory returned late or work done twice. It cannot cost a block: whatever those words decide, memory
goes back only through a claim, and the claim is modelled. The background thread's handshakes are
run under the schedule explorer against a racing slow path, and the rest are covered by tests and,
for some decisions, Kani.

### Kani

Kani proves, for every input, the arithmetic that everything else stands on: the class function,
including every size `Allocator` may pass back and C's 16-byte rule; division by a class size
without a divide; the slot tests; the layout of a reservation and the mapping from a pointer to its
region and slot; the bitmap word operations and each class's metadata layout; every word that packs
fields, and every generation's marker being one the routines reject; the decisions of the clocked
rules, as functions of the clock and the counts they read; and the twins.

### Twins

Kani, Miri and the schedule explorers cannot execute assembly. Each routine has a Rust twin written
to the same specification and selected by build configuration, run on a simulated processor that can
interrupt it before any step. Kani checks each twin against its specification on every state of a
small slab, and a test runs each twin and its routine from the same scripted states and requires the
same result and the same memory. The heap's tests run the same bodies on the process's heap with the
routines and on a simulated heap with the twins.

### Tests

- **Schedule exploration.** Explorers run the production code of a protocol on a few threads of
  which exactly one runs at a time. They switch threads only before an atomic operation and search,
  depth first, every schedule within a bound on preemptions, and a schedule may fork at any step or
  move a thread to another CPU. The scenarios follow the TLA+ models.
- **Interruption at every step.** Tests interrupt each twin before each of its steps, and the slow
  paths at each of theirs, with a signal handler that allocates, a migration and a fork, and check
  that the interruption leaves no trace. On the real routines, a mode of the soak harness puts a
  hardware breakpoint on an instruction of a sequence; the trap makes the OS abort the sequence
  there, and its handler allocates and frees before the sequence restarts.
- **Model-based testing.** The test kit drives an allocator through streams of operations and checks
  every result against a shadow model of the live blocks. One model judges `GlobalAlloc`,
  `Allocator` and the C functions, on the process's heap and on a simulated heap, across threads and
  forks. A failing stream is minimized to a script that replays, and allocators broken on purpose
  keep the model honest: each must be caught through every interface.
- **Fault injection.** The simulated OS fails any call on demand, wholly or after acting on half of
  its range. A failure to obtain memory must surface as a null return and nothing worse; a failure
  to release memory must be survivable.
- **Miri** runs the allocator's own tests on a simulated heap with the twins, under strict
  provenance.
- **Fuzzing** drives the same model from raw input, with a target for each interface.
- **The C functions** meet a table of edge cases. The C library is run against the same table, and
  broken implementations show that the cases bite.
- **Real programs** run with the preload library and must print and exit as they do without it. A
  soak runs a stress harness that checks the contents of every block, in modes that are hard on a
  per-CPU allocator: churn, hand-off between threads, fork, storms of short-lived threads, resizes,
  signals whose handlers allocate, idleness and trimming.
- **Probes** are small programs that check one assumption each about the kernel, glibc or the
  compiler, the ones under "What is trusted" among them.

### Safety ledger

An `unsafe` block in code that ships is covered by a machine-checked proof where one is practical.
Where it is not, the block has an entry in `docs/safety/<crate>/<file>-safety.md`, named after the
source file without its `.rs`. An entry states the block's preconditions, the invariants that make
them hold, and a proof on paper. A reviewer who did not write the code reviews entries adversarially
and records the commit reviewed. A script in the merge gate checks that every safety comment names
an entry and every entry is named, and lists the entries changed since their review; that list is
cleared before a release. The simulator, the twins and other test scaffolding are outside the
ledger.

### Performance

Performance claims rest on three kinds of measurement.

- **Roofline microbenchmarks** compare each fast path with floor allocators (the cheapest code that
  could serve the workload), with variants that differ from the real path in one detail, and with
  other allocators, in cycles and instructions from hardware counters.
- **Workloads**: multithreaded Rust servers and pipelines in which objects cross threads, compiler
  and data-processing runs, and established allocator benchmarks through the preload library. Rust
  workloads are measured with cralloc as the global allocator, because preloading discards Rust's
  sized frees.
- **Memory**: resident and huge-page-backed memory over time, fragmentation as resident bytes over
  live bytes, page faults, and how quickly memory returns after a peak.

Comparisons interleave the allocators, pin threads, repeat across process launches, and include a
run of one allocator against itself to expose a broken setup. `docs/research/measuring.md` has the
method.
