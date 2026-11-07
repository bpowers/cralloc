# Prior art: what others do, and what we took or rejected

cralloc has tcmalloc's front-end shape on glibc's rseq registration, mimalloc's pacing, Bonwick's
depot, Mesh's bitmaps and wasmalloc's layout-derived classes. It rejected whatever needs state in
freed memory, an atomic per object, a lock, or a write the kernel now forbids.

The path that decides the ranking is a free on another thread. Over 18 cross-thread jobs of small or
mixed sizes in which every thread allocates, mimalloc takes 1.96 times jemalloc's time (geometric
mean; geometric sd 1.39; range 0.75 to 2.79). Passing it the size changes nothing there: 1.02 times
(sd 1.03, 22 jobs). Per-CPU tcmalloc beats its own per-thread mode by 1.40 times (sd 1.16, 22 jobs).
Computed from the medians of ten rounds in the baselines of 2026-10-19 at `e96086d`.

## What we took

- **Per-CPU pointer arrays under rseq, from tcmalloc.** A free pushes a pointer, with no atomic and
  no touch of the object or its metadata, so a free on another thread looks like a local one. Built
  differently: each class's slab word is the address of the next free slot, the bounds tests are one
  mask, and the commit is a plain store, because a drainer may exchange the word between read and
  commit (`docs/design.md`, "Front end").
- **Fencing one CPU, from tcmalloc.** `membarrier` with the rseq command and a CPU number. tcmalloc
  sets a `stopped` flag first. We exchange each word with a marker carrying the process's
  generation, so a fork child can ignore what its parent left.
- **Whole-magazine exchange, from Bonwick and Adams.** It is tcmalloc's transfer cache with no lock
  and no copy: two atomic instructions a magazine. Built per cache domain plus one shared stack.
- **Pacing by bounded steps on slow paths, from mimalloc.** Built as a heartbeat paced by the bytes
  slow paths move. The first plan, one heartbeat per exchange, was dropped: a swap with a spare
  carries no maintenance now.
- **Bitmap free state, from Mesh.** Freed objects are never written, a double free is a bit test,
  and a batch is one atomic OR per word. Built without spans: one bitmap and a summary tree per
  region.
- **The address gives the class, from tcmalloc's tag bits and Scudo's regions.** Built as 4 GiB
  regions assigned to a class on demand and read from a table with one load, not one fixed region
  per class and a shift as first sketched.
- **Class and alignment from the `Layout`, from wasmalloc.** Alignment holds by construction, and
  Kani proves it. Rust computes a small object's class from the layout. A large object's class
  always comes from the address, because its slot may be bigger than it asked for.
- **Huge pages as the unit of release, from Temeraire.** Its packing problem disappears when a
  region holds one size. What remains is advice where objects are dense and purging in 2 MiB units
  below the frontier (`docs/design.md`, "Returning memory and huge pages").
- **Growing in place, from wasmalloc's losing workload.** `realloc` doubling cost it a copy at every
  class change. Built as a move into a slot with room to grow.
- **Grow on regret, shrink on a low-water mark, from everyone** ("How others size their buffers",
  below). Stock grows only on a round trip that met contention. Depot stacks shed the depth an
  interval did not need. The purge takes what two intervals' fills did not reach.
- **The working method, from wasmalloc and verified-garbage.** See
  [verification.md](verification.md).

## What we rejected

- **Indexing the caches by `mm_cid`.** Evaluated, built first, and replaced within a day
  (2026-10-17), for correctness and not for cost. A thread with no registration reads a
  valid-looking 0, and current kernels can show a garbage ID after a failed `execve`
  ([linux-rseq-contract.md](linux-rseq-contract.md), "Why we do not use `mm_cid`"). The locality it
  promises was never compared with CPU indexing on real work.
- **tcmalloc's cached slab pointer.** It writes `cpu_id_start`, which Linux 7.1 and later, under a
  full-size registration, kill the process for writing (`rseq_readonly.rs`).
- **A registration of our own on glibc.** glibc 2.35 and later got there first. We ride theirs, and
  register our own only on musl ([portability.md](portability.md)).
- **Intrusive free lists.** Writing a freed object costs its next user 7 to 27 ns across cores, and
  popping an array beats chasing a list by 1.7 cycles a pair in bursts, even with everything in L1.
- **An atomic per object on a remote free.** mimalloc's locked instruction is about a quarter of its
  cost above the floor, and the cache lines around it are the rest.
- **Per-thread heaps.** State scales with threads, moves with them, and needs code at thread exit.
- **tcmalloc's sampler and 16-bit header.** The sampler is a thread-local read-modify-write on every
  malloc, and heap profiling is a non-goal. A narrow load on the loop-carried chain costs 5 to 7
  cycles of forwarding, and one 64-bit word per class needs none. In isolation tcmalloc's sequence
  runs at 4.50 to 4.82 cycles a pair against 4.0.
- **Prefetch on the fast path.** tcmalloc's costs 0.3 cycles a pair in a tight loop, and our
  routines have none. Whether it pays on a real program was not measured.
- **Per-class out-of-line entries.** The same as generic routines in tight loops. Under LTO a `Box`
  pair through them costs 8.00 cycles against 6.80 inlined, so we inline.
- **"Sticky" `rseq_cs`, comparing instead of arming.** Never built. It saves at most one store and
  needs a shared abort handler, and arming every time makes a signal handler's own sequences
  harmless.
- **Micro-locks under time-slice extension.** No locks is a design rule, the grant is only best
  effort, and the reference kernel is built without the feature.
- **Compressed 32-bit handles, vector-compress refills.** Never built or measured. Slots stay 8-byte
  pointers, and a fill claims from one bitmap word at a time and hands out lowest address first.
- **Origin-affine return and NUMA placement.** NUMA is a non-goal. The locality rule we built is
  different: a magazine stays in the cache domain that freed it, because an object's lines live
  where it was last used.
- **Meshing.** It needs file-backed memory, which gives up anonymous huge pages, and random
  placement, which gives up locality. Size-class segregation and reuse policy have been enough.
- **`mremap` to move large blocks, `MADV_COLLAPSE`, 64 KiB multi-size huge pages.** The first adds
  only speed and splits the heap's mapping ([large-blocks.md](large-blocks.md)). The second costs
  0.1 to 0.4 ms in a slow path for little ([memory.md](memory.md)). The third is a system-wide
  setting, off on the reference host.
- **Stop-the-world resizing and learned sizes.** tcmalloc stops a CPU to move capacity. We keep
  fixed bounds plus stock ([magazines.md](magazines.md), "Sizing on evidence").
- **Exporting C++ operators.** A throwing `new` cannot unwind through Rust frames, and exporting
  only some forms is unsound (`docs/design.md`, "Interfaces").

## Sources

Claims are read in source at these commits unless tagged: **[I]** inferred, not measured; **[M]**
from memory; **[W]** from the web. Paths are inside each project. Cycle counts are from
[roofline.md](roofline.md): Ryzen 9950X, Linux 7.2.9, glibc 2.43, rustc 1.99.1.

| Project | Upstream | Read at |
|---|---|---|
| mimalloc | github.com/microsoft/mimalloc | v3.5.3 (`31d034d9`, 2026-09-16) |
| tcmalloc | github.com/google/tcmalloc | `f3c6669a` (2026-10-16) |
| Mesh | github.com/plasma-umass/mesh | `2987f88` (2026-03-25) |
| wasmalloc | github.com/bpowers/wasmalloc | `900981e` (2026-09-02) |
| jemalloc | github.com/jemalloc/jemalloc | `dev` 975f3b0 and 5.3.0 |
| Rust library | nightly 1.101 | `5d8fec066` (2026-10-16) |

The sizing section also read, on 2026-10-19: illumos `kmem.c`, FreeBSD `uma_core.c`, Linux 7.0 to
master, snmalloc, glibc 2.43, scudo, rpmalloc and Go's runtime.

## Fast paths, instruction by instruction

**mimalloc malloc, up to 1024 bytes** (`src/alloc.c`).

1. `theap = __mi_theap_default`: one TLS load, statically a read-only empty theap, so no init check.
2. `page = theap->pages_free_direct[(size+7)>>3]`: one load, no bin computation.
3. `block = page->free`, and `xused += 0x10001` on a word holding two 16-bit counts.
4. `if block == NULL` go to `_mi_malloc_generic`: the one fast-path branch.
5. `next = block->next`, a load from the object itself, then two stores.

The critical path runs through application memory, so a miss on a free block stalls the next pop.
**[I]**

**mimalloc free** (`src/free.c`). The default build has no page map: it aligns the pointer down to
256 MiB and indexes a `mi_page_t` array by slice number, then loads `page->self`. A sized free of up
to 1024 bytes skips that load, with the lookup `p & ~0xFFFF`. Then `xtid = tls_thread_id ^
page->xthread_id`, one compare for "mine, not full, no interior pointers". A local free goes on
`page->local_free`, so a freed block is not reused until the page's `free` list runs dry. A remote
free runs a compare-and-swap loop on `page->xthread_free`: it reads line 1 of the page struct, which
the owner writes on every operation, then does a locked read-modify-write on line 2, per object.
**[I]** The Rust `mimalloc` crate 0.1.52 calls `mi_free` and `mi_malloc_aligned`, and bundles 3.3.2.

**tcmalloc malloc** (`tcmalloc.cc`, `internal/percpu_tcmalloc.h`, `Pop`).

1. Size to class: `(size+7)>>3` up to 1024, then one byte load from `class_array_`.
2. Sampler: `bytes_until_sample_ -= size+1`, a TLS read-modify-write on every malloc.
3. rseq prologue: store this sequence's descriptor into `__rseq_abi.rseq_cs`.
4. `scratch = tcmalloc_slabs`, whose top 4 bytes overlap `cpu_id_start`; `btrq $63; jnc slow`.
5. `current = movzwq header[class]`; `result = slabs[current-1]`; `testb $1`, the begin mark.
6. `subl $0xffff0001, header[class]`: one 32-bit read-modify-write that commits.
7. Outside the sequence: `prefetchw slabs[current-3]`, `prefetcht0` of the next object.

Neither loop-carried chain goes through object memory, and the 16-bit load feeds only the slot
address. **[I]**

**tcmalloc free** (`do_free`, `Push`). One address-bit compare rejects null, sampled, cold and
corrupt pointers together. A sized free gets its class from `class_array_` with no span metadata
touched; an unsized one makes two dependent loads in a radix page map. `Push`: prologue, cached
slabs, `current = movzwq tmp`, `addl $0xffff0001, tmp`, `jnc overflow`, `slabs[current] = item`,
`header = tmp`. A preempted push leaves a dead write above `current`.

**What each costs**, cycles per allocate and free, pinned, fat LTO:

| Shape | Pair | Note |
|---|---:|---|
| Bare magazine, everything hoisted | 2.0 | the floor |
| cralloc, inlined, per-CPU under rseq | 4.2 to 5.3 | `docs/design.md`, "What the hardware says" |
| tcmalloc's `Pop` and `Push`, in isolation | 4.50 to 4.82 | not the whole library |
| glibc through `GlobalAlloc` | 11 | |
| mimalloc through `GlobalAlloc` | 12.4 to 13.7 | 10.3 with sized free |
| jemalloc through `GlobalAlloc` | 12.1 | |

Any path through two out-of-line calls costs at least 8.0, because a call and return take 4 whatever
the body.

## tcmalloc's rseq mechanics

- **Cached slab pointer** (`internal/percpu.h`). `tcmalloc_slabs` sits 4 bytes before `__rseq_abi`,
  so its top 4 bytes are `cpu_id_start`. The cached value has bit 63 set, and the kernel clears it
  by writing a CPU number. It removes the CPU-number load, and it depends on the kernel rewriting
  the field on every reschedule and tolerating user writes. Neither holds under glibc's registration
  on Linux 7.1 and later.
- **Layout.** Each CPU has 4-byte class headers `{u16 current; u16 remaining_capacity}`, then
  pointer arrays, each after a slot holding a pointer to itself with the low bit set, both underflow
  sentinel and prefetch target.
- **Cross-CPU.** Set `stopped[cpu]`, then `membarrier(PRIVATE_EXPEDITED_RSEQ, FLAG_CPU, cpu)`.
  `mm_cid` is behind a test-only experiment, and with it the real CPU is unknown, so it fences all
  of them.
- **Registration.** tcmalloc defines its own 32-byte `__rseq_abi` and registers it with a raw system
  call, which glibc's registration refuses **[I]**. Its tests expect working rseq only with
  `GLIBC_TUNABLES=glibc.pthread.rseq=0`. Our runner confirms the outcome: the same library reports
  per-thread mode without the tunable and per-CPU mode with it. So a benchmark that does not set it
  measures per-thread tcmalloc, and the system `libtcmalloc.so` is gperftools, a different
  allocator.

## Slow paths and pacing

**mimalloc.** Everything hangs off `page->free == NULL` (`src/page.c`). `_mi_malloc_generic` looks
at the first page in the bin queue and swaps `local_free` into `free` in O(1). Otherwise it walks
the queue, taking `xthread_free` with one compare-and-swap and walking it, one pointer chase per
remotely freed object. It considers up to 4 candidates and prefers fuller pages, and full pages
beyond 2 are abandoned. Extension builds at most 8 KiB of blocks at a time. A fresh page first tries
to reclaim an abandoned one. Every 1000 generic calls it runs retired-page collection, and every
10000 a full collect with arena purge. That is what "paced" means: bounded work per slow-path entry,
at a rate set by free-list length, with no background thread. Statistics ride on the 16-bit
allocation counter, so there is no per-malloc sampling cost.

**tcmalloc.** A front-end miss moves up to 128 pointers a batch to or from the transfer cache, a
per-class array under a spinlock. Below it, a central free list has a lock per class and spans in 8
lists by allocated count. A background thread does release, cache shuffling and slab resizing.

## Cross-thread free

- **mimalloc:** one compare-and-swap per object. v3 abandons full pages at once, so frees into them
  take the multithreaded path, and the first free that finds a page unowned claims and collects it.
- **tcmalloc:** a push to the current CPU's array. Cost appears only at overflow: one transfer-cache
  lock per batch, and a symmetric refill on the producer. Objects recirculate without being
  re-sorted by address, which is why its span lists exist. **[I]**
- **snmalloc [W]:** a per-thread outbox keyed by owning allocator, appended to the owner's inbox
  with one exchange. BatchIt (ISMM 2024) adds a cache keyed by slab, so a message is a ready-made
  free-list segment. Reported on `xmalloc-testN`: mimalloc 27.6% less time, snmalloc 14.7% faster.
- **jemalloc [M]:** frees go to the freeing thread's cache, a pointer stack, and a flush locks each
  destination bin once per group. `tikv-jemallocator` frees with the sized `sdallocx`. jemalloc
  gives a thread that only frees no cache, which makes it two to three times slower there:
  mimalloc's time over jemalloc's is 0.37 to 1.02 on the four free-only jobs, against the 1.96
  above.

**Why tcmalloc and jemalloc were expected to beat mimalloc on multithreaded Rust.** The project
began from internal Rust services where `tikv-jemallocator` beat an allocator of mimalloc v3's
shape. Our microbenchmarks agree, and real servers split. In the standings of 2026-10-28 at
`2ff7322`, cycles a request at 70% load, mimalloc is 3 to 4% ahead of jemalloc on the Connect log
services and 3 to 13% behind on the order service and the log store. The hypotheses we started with:

| Hypothesis | Status |
|---|---|
| Per-object against per-batch cross-thread cost | Supported. One locked instruction per free is 3.5 ns, about a quarter of mimalloc's excess, and the rest is cache lines. Across L3s it costs 2.1 to 2.7 times jemalloc on a pipeline |
| Free touches shared metadata because the crate calls unsized `mi_free` | Refuted across threads. The sized call helps one thread alone (1.9 to 2.1 ns against 2.6 to 2.7), and 1.02 times over 22 cross-thread jobs |
| v3's abandoned-page path | Not tested. `MIMALLOC_PAGE_FULL_RETAIN=-1` and `MIMALLOC_PAGE_RECLAIM_ON_FREE=1` are the experiment |
| Reuse distance from `local_free` | Not isolated. Our churn lead is consistent with it |
| Thread count and migration | Not tested. Runs are pinned |
| Back end: mimalloc purges after a delay, tcmalloc keeps huge pages | Not tested as a cause |

## Back ends

- **mimalloc.** Arenas of 64 KiB slices, reserved 1 GiB at a time, with all state in atomic bitmaps.
  Pages are 64 KiB, 512 KiB or 4 MiB. Fragmentation control is "prefer fuller pages" plus
  abandonment. The purge delay is 1 s, times 4 in arenas.
- **Temeraire.** Choose the unit of release, the 2 MiB huge page, and pack so that whole huge pages
  are dense or empty. Ordering by longest free range avoids spoiling long runs, and counting
  allocations predicts emptiness better than counting pages.
- **wasmalloc.** One last-in first-out list beat mimalloc's two lists once measured. 16-bit header
  fields and a cold call on a common transition were expensive. Footprint was one page per touched
  bin.

## What Rust tells the allocator

From `core/src/alloc/mod.rs` at nightly 1.101.

- `deallocate` must get a layout that "fits": the same alignment, and any size from the request to
  the returned length. So the class function must be constant over that interval, which returning
  exactly the class size satisfies when classes are monotone.
- `Allocator` is stable from 1.100 and gated on stable 1.99.1, so the `allocator-api` feature needs
  nightly until then. `GlobalAllocator` is still unstable, so a global allocator is `GlobalAlloc`:
  the exact layout on `dealloc`, and `realloc` with unchanged alignment.
- `Vec<u8, A>` ignores the excess length. With a 64-byte block for a 10-byte request, `capacity()`
  is 10 and `deallocate` receives 10 (`vec_excess.rs`). So `dealloc` sees the requested size, not
  the class size.

| Rust gives | It buys |
|---|---|
| size and alignment on dealloc | the class with no lookup, a compile-time constant for `Box`, `Arc` and most nodes |
| alignment on alloc | no over-allocation or interior pointers; 24, 40 and 56-byte classes where C must round to 16 |
| old and new layout on resize | the in-place decision for a small object, with no metadata read |
| zeroed variants | no `memset` for large memory known to be zero; small classes do not track it |

C forces: `free(p)`, `realloc(p, n)` and `malloc_usable_size(p)` arrive with no size, so the class
comes from the region table, one load. `malloc` returns 16-byte alignment and `malloc(0)` a unique
pointer, the `memalign` family is freed with plain `free`, and there is `errno`, fork, and calls
before constructors run. Both ABIs share one heap, and C may free what Rust allocated.

## How others size their buffers

Every adaptive mechanism found has one shape: a regret signal grows the buffer by a step, idleness
measured as a low-water mark over a window shrinks it, and a fixed bound clamps it.
[magazines.md](magazines.md), "Sizing on evidence", says what this means for cralloc.

| System | Grows on | Shrinks on | Bound |
|---|---|---|---|
| tcmalloc per-CPU | a miss; past two batches, an overflow then an underflow | quiescent ticks, stealing a slot at a time | 1.5 MiB per CPU, zero-sum |
| tcmalloc HugeCache | a miss: limit = 1.1 x the demand dip over 1 s | a minimum over 2 s above a fifth of the limit | floor of 10 |
| jemalloc `dev` tcache | a refill of an empty bin doubles the fill | a GC visit with low water above zero halves it | 2 x slab objects, 20 to 200 |
| jemalloc decay | reuse only | a 10 s smootherstep over 200 epochs | none |
| Bonwick kmem | more than 3 failed `tryenter`s in 15 s | never; the depot's working set is reaped under pressure | 9 sizes |
| FreeBSD UMA | +1 each time the zone lock is held | -1 on each reclaim | 254 |
| Linux per-CPU pages | a scaled batch on each zone refill | an eighth a second at most | `high_min` to managed/8 |
| Go scavenger | heap goal x 1.1 | the heap goal, at 1% of CPU | GOMEMLIMIT |
| cralloc stock | a round trip to the depot that met contention | the low-water mark of an interval | the CPU's spare budget |
| cralloc depot shed | a push | the low-water mark of an interval | the domain cap; the shared target |
| cralloc frontier | a fill that reaches higher | pages above two intervals' frontiers, down to the floor | the cap, charged to growth |

Lessons:

- **The shrink must be slower than the growth,** or a buffer that is exactly right looks idle.
  tcmalloc says so of its caches and HugeCache; Linux grows by a batch per refill and decays an
  eighth a second.
- **Contention is the cost signal for a shared tier,** because it is free to observe: Bonwick and
  UMA on their locks, our stock on a failed compare-and-swap.
- **A round trip is one useful signal, not the rule.** A miss on a one-way flow says nothing a
  bigger buffer would fix, but only tcmalloc's growth past two batches asks for a round trip. Linux
  grows on a plain refill by design, since a CPU that only allocates holds no idle pages.
- **One-way ratchets fail.** glibc's mmap threshold only grows, and rpmalloc dropped its 1.3
  thread-cache rule in 1.4.
- **Learned models did not survive.** LLAMA's lifetimes became a fixed span-capacity threshold in
  tcmalloc (+1.0% throughput, -0.8% memory, ASPLOS 2024). No published allocator autotuner runs in
  production.
- **One knob works when it is a ratio with a meaning, and a hard cap stands behind it.** GOGC is a
  heap ratio, the memory limit was added because a ratio cannot stop a spike, and a 50% CPU cap
  stops the limit spiralling.
- **Nobody derives thresholds from costs.** tcmalloc's source calls its scores "intuition", UMA's
  constants are "somewhat arbitrary", Linux's decay is "kind of arbitrary".

**tcmalloc** (`cpu_cache.h`, `background.cc`). All resizing runs in one background loop every
second. A miss grows a class by one below two batches, and refills ramp by `batch << successive`, up
to half the capacity. A well-sized list looks idle and is shrunk, by stealing once its quiescent
ticks reach a size-dependent score. Every 2 s the 5 classes most often refused room grow at the
others' expense, and every 5 s the 5 most-missing CPUs steal 5% of the limit from others. Every 30 s
it drains a CPU unchanged and missless, which can fire on a busy CPU whose last-in first-out churn
never misses. Every 29 s the slab size may change, which stops and drains every CPU and "erases any
learned information". Fleet results: the default cache halved from 3 to 1.5 MiB with no loss and
fleet memory fell 1.9% (ASPLOS 2024); Temeraire cut overhead from 15.0 to 11.2% in the A/B (OSDI
2021).

**jemalloc.** `dev` replaced eight tcache options with rules. A refill of an empty bin doubles the
fill, to half the capacity; a GC visit where the bin never emptied halves it, to 2. GC runs every 64
KiB of the thread's traffic, at most every 10 ms. A consumer converges to flushing nearly the whole
bin, under the bin lock, at every overflow. Decay fails on phases longer than its window.

**Bonwick and Adams 2001** (illumos `kmem.c`). Every 15 s, if the depot lock's failed `tryenter`s
rose by more than 3, it purges every CPU's magazines and steps up a table of 1 to 143 rounds. It
grows only, and each resize discards every cached object. Each depot list keeps its minimum, and a
reap frees what the whole previous interval did not use. That runs under memory pressure, so idle
depot memory is otherwise kept.

**Linux.** Per-CPU pages (2023): `high` grows on each zone refill, and a free that finds the list at
`high` raises it to hold the run. Kbuild ran 3.5 to 5% faster, with zone-lock contention down from
11 to 14% to 0.5 to 1%, for up to 18.4% more memory. Sheaves (2025, default in 7.0): 4 to 60 objects
by size, a main and a spare per CPU, a barn of 10 full and 10 empty per node. Nothing adapts.

**Others.** mimalloc 3.5 is all fixed: purge 4 s after an arena's first range is scheduled, so a
range freed just before it goes at once. An unmerged snmalloc branch has a large-object cache whose
budget grows by the excess of misses over stale flushes and shrinks by half the excess of flushes.
glibc 2.43 raises its mmap threshold to the size of any mmapped chunk freed, up to 32 MiB, and never
lowers it.

**Managed runtimes.** GOGC works because GC CPU per byte allocated is about scan work over headroom,
an exchange rate users can reason about. Go's scavenger retains the goal plus 10%, works at 1% of
CPU, and skips chunks dense in the current or last cycle. G1 doubled `GCTimeRatio` from 12 to 24
once shrinking became real, because the old value shrank heaps too far. TMO (Meta, ASPLOS 2022)
reclaims `current * 0.0005 * max(0, 1 - PSI / 0.1%)` every 6 s, one setting for all services, saving
20 to 32% of memory.

## Open questions

- **Would `mm_cid`'s thread-cache locality help real work when threads are fewer than CPUs?** Its
  ownership rules were read in the kernel's source and stress-tested for safety. The locality was
  never measured.
- **The mimalloc hypotheses marked "not tested" above.** None now steers the design.
- **Second readers.** jemalloc's cross-thread details are from memory, and the figures from ASPLOS
  2024, OSDI 2021, LLAMA, TMO and snmalloc's branch were read once.
