# Prior art: mimalloc, tcmalloc, Mesh, wasmalloc

A source-level reading of the allocators asmalloc borrows from, done on 2026-10-16, with the
kernel and Rust facts that constrain a new design and the design ideas that came out of it.
`docs/design.md` is the design; this document is the evidence behind it.

## Summary

- **tcmalloc's front end is the right shape, and its implementation is closed to us.** A free
  pushes a pointer onto a per-CPU array inside a restartable sequence: no atomics, no object
  or span metadata touched, and a cross-thread free looks exactly like a local one. But
  tcmalloc registers its own rseq area, which glibc 2.35 and later pre-empt, and its
  cached-slab-pointer trick writes a field that Linux 7.x, under glibc's registration,
  kills the process for writing.
- **mimalloc's pacing is the right idea, and its cross-thread free is its weak point.** One
  fast-path branch ("the free list is empty") drives all maintenance in bounded steps. A free
  from another thread costs one locked instruction per object plus a miss on a cache line the
  owner writes on every operation, and v3 abandons full pages, which sends more frees down
  that path. This is the leading explanation for mimalloc losing to jemalloc and tcmalloc on
  multithreaded Rust services; it is a hypothesis, not a measurement.
- **`mm_cid` gives a per-CPU cache the locality of a thread cache.** Since the 2025 kernel
  rework a task owns its concurrency ID while threads do not outnumber CPUs, and IDs become
  per-CPU only when oversubscribed (from the kernel's documentation and mailing list; not
  tested here).
- **Rust tells the allocator the size and alignment on every free.** The size class is then a
  function of the `Layout`, often a compile-time constant, and free needs no pointer-to-
  metadata lookup. `Vec` does not use the excess capacity an `Allocator` returns, so
  `dealloc` sees the requested size and the class function must be constant from the
  requested size up to the class size.
- **Bitmap spans (from Mesh) and layout-derived geometry (from wasmalloc) carry over.**
  Meshing itself does not: see "Ideas considered".

## Sources and conventions

Paths are relative to `third_party/`, a directory of local reference checkouts that is not
part of this repository. To follow a reference, check out the project at the commit below.

| Project | Upstream | Read at |
|---|---|---|
| mimalloc | https://github.com/microsoft/mimalloc | v3.5.3 (`31d034d9`, 2026-09-16) |
| tcmalloc | https://github.com/google/tcmalloc | `f3c6669a` (2026-10-16) |
| Mesh | https://github.com/plasma-umass/mesh | `2987f88` (2026-03-25) |
| wasmalloc | https://github.com/bpowers/wasmalloc | `900981e` (v0.1.1 + 2, 2026-09-02) |

Rust standard library text was read from nightly 1.101 (`5d8fec066`, 2026-10-16) under
`~/.rustup/toolchains/nightly-*/lib/rustlib/src/rust/library/`; the stable 1.99.1 toolchain
on the host has no `rust-src`.

Each claim carries a tag:

- **[S]** read in source at the commit above.
- **[I]** inferred from source, not measured.
- **[X]** checked by a small experiment on the reference host (Ryzen 9950X, Linux 7.2.9,
  glibc 2.43, rustc 1.99.1); the probes are in `bench/probes/`.
- **[W]** from the web, link given.
- **[M]** from memory, unverified.

Cycle counts quoted from `docs/research/zen5-roofline.md` are marked as such.

## 1. Fast paths at instruction level

### 1.1 mimalloc malloc (small, up to 1024 bytes)

Source: `mimalloc/src/alloc.c:53-142` (`mi_page_malloc_zero`), `:153-183`
(`mi_theap_nonnull_xmalloc_small_zero`), `include/mimalloc/internal.h:677-682`,
`include/mimalloc/prim-tls.h:249-260`. [S]

1. `theap = __mi_theap_default`: one TLS load. It is statically initialised to a read-only
   empty theap, so there is no init check (`prim-tls.h:247-260`, `internal.h:652`).
2. Compare `size <= MI_SMALL_SIZE_MAX` (1024 bytes: `include/mimalloc.h:123-124`).
3. `page = theap->pages_free_direct[(size+7)>>3]`: one load, no bin computation
   (`internal.h:679-681`). The `clz`-based `mi_bin` (8 exact word bins, then 4 sub-bins per
   doubling: `src/page-queue.c:63-97`) runs only in slow paths.
4. `block = page->free` (load), `xused = page->xused` (load), `xused += 0x10001`. The used
   count and the allocation count are two 16-bit fields in one word, so one add updates both
   (`types.h:417-438`, `alloc.c:63-65`).
5. `if (block == NULL)` go to `_mi_malloc_generic`: the single fast-path branch.
6. `next = block->next`: a load from the object itself.
7. `page->free = next; page->xused = xused`: two stores.

All state is thread-local: the TLS word, one line of the direct table, the first cache line
of `mi_page_t`, and the first word of the block.

Loop-carried chains [I]: `page->free` store, next malloc's `page->free` load, then the
`block->next` load (4 cycles in L1, or a miss), then the store. The allocator's own critical
path runs through application object memory, so a cache miss on a free block stalls the next
pop. The second chain is the `page->xused` load, add, store.

`mi_page_t` layout (`types.h:467-500`, with `MI_PAGE_META_IS_ALIGNED`): `self`, `xthread_id`,
`free`, `xused`, `local_free`, `block_size`, `page_offset` (8 bytes each),
`capacity/reserved/slice_pcommitted` (6), `retire_expire` (1), `free_is_zero` (1): 64 bytes.
`xthread_free` starts the second line (`:488`).

### 1.2 mimalloc free

Source: `src/free.c:192-248` (pointer to page), `:261-287` (`mi_free_nonnull`), `:27-57`
(local), `:63-96` (other thread), `:334-369` (`mi_free_size`). [S]

Finding the page has three build modes:

- Default build (`MI_FREE_IS_CHECKED` off: `CMakeLists.txt:484-490`;
  `MI_PAGE_META_IS_ALIGNED=1`: `types.h:169-176`): no page map. Align the pointer down to
  256 MiB (`MI_PAGE_META_ALIGNMENT`, `types.h:268-270`), index a `mi_page_t` array at the
  arena start by slice number (`internal.h:802-814`), then load `page->self`
  (`free.c:229-239`; needed for pages that span several slices).
- Sized free of a small object: `mi_free_size(p, size)` with `size <= 1024` calls
  `mi_free_small` (`free.c:360-368`). With `MI_PAGE_META_SMALL_IS_ALIGNED` the page struct
  sits at the start of the 64 KiB small page, so the lookup is `p & ~0xFFFF` with no `self`
  load (`free.c:203-208`, `types.h:181-189`). wasmalloc uses the same mask.
- Secure or checked builds: a two-level page map (`internal.h:780-795`).

Then (`free.c:266-286`) `xtid = tls_thread_id ^ page->xthread_id`. The thread id is the
thread pointer (`prim-tls.h:170-176`), and page flags live in the low two bits of
`xthread_id`, so one compare covers "mine, not full, no interior pointers".

- `xtid == 0` (local): `block->next = page->local_free; page->local_free = block; xused--`,
  and a retire check when `used` reaches zero (`free.c:42-56`). Frees go to `local_free`, not
  `free`: a freed block is not reused until the page's `free` list runs dry.
- Another thread (`(xtid & 3) == 0`): `mi_free_block_mt` (`free.c:63-96`) writes
  `block->next` and runs a CAS loop on `page->xthread_free`, one `lock cmpxchg` per object.
  If the CAS also flipped the owned bit (the page was abandoned and unowned) it calls
  `mi_free_try_collect_mt` (`free.c:520-555`).

Cost of a cross-thread free [I]: it reads line 1 of the page struct (`self`, `xthread_id`),
which the owner writes on every malloc and free because `free`, `xused` and `local_free` are
on the same line. That is a coherence miss for the freeing thread and an upgrade miss for the
owner's next operation, followed by a locked read-modify-write on line 2. Per object.

The Rust `mimalloc` crate takes the unsized path: `dealloc` calls `mi_free(ptr)` and `alloc`
calls `mi_malloc_aligned(size, align)` (crate 0.1.52, `src/lib.rs:50-61`; its
`libmimalloc-sys` 0.1.49 bundles mimalloc 3.3.2 by default and 2.3.2 behind the `v2`
feature; read in the local cargo registry). [S]

### 1.3 tcmalloc malloc

Source: `tcmalloc/tcmalloc.cc:1329-1367` (`fast_alloc`), `sizemap.h:143,239-296`
(`GetSizeClass`), `sampler.h:163-174`, `internal/percpu_tcmalloc.h:870-913` (`Pop`). [S]

1. Size to class: `ClassIndexMaybe` (`(size+7)>>3` up to 1024, `(size+127+(120<<7))>>7`
   above; `sizemap.h:69-91`), then one byte load from `class_array_` (`sizemap.h:131`).
2. Sampler: `bytes_until_sample_ -= size+1` with a borrow check, a TLS read-modify-write on
   every malloc (`sampler.h:163-174`).
3. rseq prologue: store the address of this sequence's `rseq_cs` descriptor into
   `__rseq_abi.rseq_cs` (`percpu_tcmalloc.h:536-556,597-646`).
4. `scratch = tcmalloc_slabs` (TLS, 8 bytes whose top 4 overlap `__rseq_abi.cpu_id_start`);
   `btrq $63`; `jnc slow` (`:879-883`).
5. `current = movzwq header[class]`, a 16-bit load (`:885`).
6. `result = slabs[current-1]`; `testb $1` (the begin mark); `jnz underflow` (`:886-888`).
7. `next = slabs[current-2]`, for the prefetch.
8. `subl $0xffff0001, header[class]`: one 32-bit read-modify-write that decrements `current`
   and increments `remaining_capacity`. This is the commit (`:889-893`).
9. Outside the sequence: `prefetchw slabs[current-3]`, `prefetcht0 next` (`:905-908`,
   rationale `:852-866`).

State: one TLS cache line holding the sampler, the cached slabs pointer and `__rseq_abi`
(`percpu.h:182-188`, checked at `percpu.cc:143-165`); the `class_array_` byte; the per-CPU
header word; the per-CPU pointer array.

Loop-carried chains [I]: the header read-modify-write (32-bit) and the sampler
read-modify-write. Neither goes through object memory, so object misses do not stall the
allocator, and the next object is prefetched.

The 16-bit `current` load raised the question of whether this sequence is off the Zen 5
roofline, where 8- and 16-bit zero-extending loads forward from a store in 7 cycles against
zero for 32- and 64-bit. Measured, it is not far off: tcmalloc's exact sequence runs at 4.5
to 4.8 cycles per inlined alloc+free pair against 4.0 for a single 64-bit word per class
(`docs/research/zen5-roofline.md`). The `movzwq` feeds only the slot address and is not on
the loop-carried chain. [I]

### 1.4 tcmalloc free

Source: `tcmalloc.cc:827-862` (`do_free`), `:936-978` (`do_free_with_size`), `:990-1052`,
`:613-630` (`FreeSmall`), `percpu_tcmalloc.h:739-779` (`Push`). [S]

- Address tag check: `(ptr & kNormalOrBadDeallocationMask) == kNormalMask`
  (`tcmalloc.cc:677-694,840,950`). Three address bits at bit 42 encode normal, sampled, cold
  and partition (`internal/memory_tag.h:28-52`); null, sampled, cold and corrupt pointers all
  fail this one compare.
- Sized: class from `class_array_`, with no span metadata touched. Unsized:
  `pagemap().sizeclass(PageIdContaining(ptr))` (`tcmalloc.cc:855`; radix lookup
  `pagemap.h:199-229,276`), two dependent loads into shared read-mostly memory.
- `Push`: rseq prologue; load cached slabs; `btrq`/`jnc`; `tmp = header` (32-bit);
  `current = movzwq tmp`; `addl $0xffff0001, tmp`; `jnc overflow`; `slabs[current] = item`;
  `header = tmp` (the commit). Then prefetch `slabs[current+1]`. The carry is clear only when
  `remaining_capacity` was 0, which is why `current == 0xffff` is reserved
  (`:413-417,756-764`).

A free never reads or writes the freed object and never touches span metadata. Cross-thread
frees are indistinguishable from local frees at this layer.

### 1.5 tcmalloc's rseq mechanics

- **Cached slab pointer** (`percpu.h:125-188`): `tcmalloc_slabs` is placed 4 bytes before
  `__rseq_abi` (`TCMALLOC_RSEQ_SLABS_OFFSET -4`, `percpu.h:26`) by hand-written layout so its
  top 4 bytes are `cpu_id_start`. The cached value has bit 63 set; when the kernel writes a
  CPU number into `cpu_id_start` the bit clears and the next fast path falls to the slow
  path. This removes the cpu-id load, the shift load, the shift and the add from the fast
  path. It depends on the kernel rewriting `cpu_id_start` on every reschedule and tolerating
  user writes to it; section 6 shows neither holds under glibc's registration on Linux 7.x.
- **Slab layout** (`percpu_tcmalloc.h:132-140,1094-1179`): one contiguous region of
  `num_cpus << shift` bytes. Per CPU: `NumClasses` 4-byte headers, then pointer arrays with
  static per-class `begin` offsets (`begins_`, the same for all CPUs). Each class array is
  preceded by a slot holding a pointer to itself with the low bit set: it is both the
  underflow sentinel and a valid prefetch target (`:442-448,1151-1158`).
- **Header** (`:413-435`): `{u16 current; u16 remaining_capacity}`, both in pointers from the
  CPU region start. `end` is implicit, so grow and shrink only edit `remaining_capacity`.
- **Single-store commit**: Pop commits with the `subl` on the header; Push stores the item,
  then stores the header. A preempted Push leaves a dead write above `current` that is
  overwritten later (`docs/rseq.md:184-190`).
- **Abort**: the kernel moves the instruction pointer to a trampoline preceded by the 4-byte
  signature (`0x53053053` on x86-64, `percpu.h:65`). The trampoline is `jmp 3f`, which
  re-stores `rseq_cs` and re-runs from the top (`percpu_tcmalloc.h:622-646`). Inputs are
  still in registers because the sequence has not clobbered them.
- **Migration and first use**: `CacheCpuSlabSlow` (`:1214-1268`) sets `tcmalloc_slabs` to the
  mask, reads the virtual CPU (`mm_cid` or `cpu_id`, `percpu.cc:97-126`), computes
  `slabs + (vcpu << shift)`, stores it with `StoreCurrentCpu` (an rseq sequence that commits
  only if not rescheduled: `:676-727`), then checks `state_[vcpu].stopped` and that
  `slabs_and_shift_` did not change.
- **Cross-CPU operations** (`:260-287`, `docs/rseq.md:388-423`): set `stopped[cpu]`, then
  `FenceCpu`, which is `membarrier(MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ,
  MEMBARRIER_CMD_FLAG_CPU, cpu)` (`percpu.cc:350-362`). That interrupts the CPU and aborts
  any running sequence, after which the remote slab is quiescent. With virtual CPUs the real
  CPU is unknown, so it fences all CPUs (`percpu.cc:388-393`).
- **Virtual CPUs**: `mm_cid` use is behind a test-only experiment (`percpu.cc:174-184`). The
  32-byte struct also carries Google's out-of-tree `vcpu_id`
  (`linux_syscall_support.h:35-46`).
- **Dynamic slab size**: shift 14 to 18, that is 16 KiB to 256 KiB per CPU
  (`cpu_cache.h:82-83`). `slabs_and_shift_` packs pointer and shift in one atomic word
  (`percpu_tcmalloc.h:366-401`). `ResizeSlabs` stops all CPUs, swaps, and drains the old slab
  (`:185-205`, `cpu_cache.h:2469-2545`).
- **Capacity management** (`cpu_cache.h:1237-1303`): grow a class by 1 until it reaches two
  batches, then by a batch only when an overflow is followed by an underflow. Bytes come from
  a per-CPU budget `resize_[cpu].available` (`:1347-1371`; limit `kMaxCpuCacheSize` 1.5 MiB:
  `common.h:142`). The refill and drain amount ramps as `batch << successive_misses` up to
  half the capacity (`:1202-1234`). A background thread steals capacity between classes and
  between CPUs with a clock-like per-class tick (`:1881-2047`), resizes slabs, and plunders
  the transfer cache (`background.cc:154-187`).

## 2. Slow paths and pacing

### 2.1 mimalloc

Everything hangs off the one fast-path branch, `page->free == NULL`. [S]

- `_mi_malloc_generic` (`page.c:1326-1358`): while `++generic_count < 1000` and the request
  is small, look at the first page in the bin queue; `mi_page_free_quick_collect`
  (`:376-385`) swaps `local_free` into `free` in O(1). The thread-free list is not collected
  here.
- Otherwise `mi_page_queue_find_free_ex` (`:944-1059`) walks the bin queue. Per page,
  `_mi_page_free_collect` (`:387-416`) takes `xthread_free` with one CAS (`:357-372`) and
  walks it to count and append (`:321-354`), one pointer chase per remotely freed object.
  Full pages are abandoned beyond `page_full_retain` = 2 (`:972-981`, `options.c:167`). It
  considers up to `page_max_candidates` = 4 candidates and prefers fuller pages
  (`:986-1002`, `options.c:168`); failing that it extends the chosen page.
- Lazy extension: at most `MI_MAX_EXTEND_SIZE` = 8 KiB of blocks per extension
  (`page.c:799,826-838`) with on-demand commit (`:843-869`). The comment at `:808-810`
  records that bump allocation was tried and did not help.
- Fresh page: first try to reclaim an abandoned page of the same bin from the arena bitmap
  (`arena.c:1142-1148,722-776`), else carve slices.
- Heartbeat: every 1000 generic calls, the deferred-free callback and retired-page
  collection (`page.c:1213,1237-1241`); every `generic_collect` = 10000, a full theap collect
  including arena purge (`page.c:1229-1233`, `options.c:165`, `theap.c:124-151`).
- Retire: an emptied page is kept for 16 heartbeat cycles (4 for larger blocks) if it is the
  only page or small (`page.c:595-638`), so alloc and free at a page boundary do not thrash.
- Statistics and sampling ride on the 16-bit allocation counter and are reconciled in the
  slow path (`page.c:240-275,287-314`). There is no per-malloc sampling cost, unlike
  tcmalloc's sampler.

This is what "paced" means: the maintenance done per slow-path entry is bounded (one swap, a
bounded queue walk, or 8 KiB of list building), and slow-path entries occur at a rate set by
free-list length. Maintenance is amortised deterministically over allocations with no
background thread.

Structure in v3: a shared `mi_heap_t` owns per-thread `mi_theap_t`s with 75 page queues and
the direct table (`types.h:611-658`). Arenas keep atomic bitmaps for free, committed, dirty
and purge slices and per-bin abandoned-page bitmaps (`types.h:787-822`). Pages are 64 KiB,
512 KiB or 4 MiB (`types.h:249-251`) holding objects up to 10 KiB, about 84 KiB and 512 KiB
(`types.h:513-516`). The purge delay is 1000 ms (`options.c:145`); THP mode 2 allows huge
pages and purges in 2 MiB units (`options.c:103-108`).

### 2.2 tcmalloc

- A front-end miss calls `Refill` or `DeallocateSlow`, which move up to `kMaxObjectsToMove`
  = 128 pointers per batch (`common.h:207`, `cpu_cache.h:1150-1180,2090-2122`) to or from
  the transfer cache.
- Transfer cache: per class, an array of pointers under a spinlock
  (`transfer_cache_internals.h:100,157-194`), 16 to 64 batches (`:51-52`), with low-water
  plundering back to the central list (`:218-247`). There is an optional variant sharded per
  L3 (`transfer_cache.h:85-86`).
- Central free list: a lock per class. Spans sit in 8 lists bucketed by allocated count so
  allocation prefers the fullest spans (`central_freelist.h:74-76,273-300,387-427`).
- Back end: Temeraire (`docs/temeraire.md`). HugeAllocator hands out unbacked address space,
  HugeCache holds backed whole hugepages, HugePageFiller packs sub-hugepage spans (ordered by
  longest free range, then by allocation count), and HugeRegion serves requests slightly
  over a hugepage from 1 GiB regions.
- A background thread does release, cache shuffling and slab resizing
  (`background.cc:63-228`).

## 3. Cross-thread free and producer/consumer flows

### What each allocator does

- **mimalloc** [S]: one CAS per object on the owning page's `xthread_free`
  (`free.c:82-87`). v3 abandons full pages immediately (`page.c:554-563`). An abandoned page
  has thread id 0 or 4, so every free into it, including from the thread that allocated it,
  takes the multithreaded path. The first free that finds it unowned claims it and runs
  `mi_free_try_collect_mt` (`free.c:520-555`), which collects and then does one of: free the
  page; reclaim it (only into the originating theap by default: `page_reclaim_on_free` = 0,
  `options.c:166`, `free.c:493-503`); re-abandon it as "mapped" once it is under 7/8 used
  (`free.c:421-430`, `arena.c:1382-1404`: an arena bitmap set plus an atomic increment of
  `heap->abandoned_count`); or release ownership with another CAS (`free.c:435-458`).
- **tcmalloc** [S]: push to the current CPU's array. Cost appears only at overflow: one
  transfer-cache lock per batch of up to 128 objects, and a symmetric refill on the producer.
- **snmalloc** [W, BatchIt, ISMM '24]: a per-thread outbox hash table keyed by owning
  allocator. When the outbox is large enough each chain is appended to the owner's
  multi-producer inbox with one atomic exchange, and the owner drains its whole inbox
  occasionally (linear work, random cache traffic, delayed reuse). BatchIt adds a small
  per-thread cache keyed by destination slab, so a message is a ready-made free-list segment
  with its length in the head and the receiver does constant work per batch. Reported:
  mimalloc `xmalloc-testN` 27.6% less time, snmalloc 14.7% faster, `msgpass` up to 20 to 30%.
  https://www.microsoft.com/en-us/research/uploads/prodnew/2024/05/preprint_batchit.pdf
- **jemalloc** [M]: frees go to the freeing thread's tcache bin, a pointer stack. A flush
  locks each destination arena bin once per group. The tcache is garbage collected on a
  ticker. `tikv-jemallocator` frees with the sized `sdallocx`.

### Why tcmalloc and jemalloc may beat mimalloc on multithreaded Rust

The motivating observation is from internal Rust services, where `tikv-jemallocator`
outperformed an allocator with mimalloc v3's architecture and the cross-thread free path was
the suspected cause. It has not been reproduced here, and no public measurement was found.
The hypotheses below are all [I] and each names its test.

- **H1. Per-object against per-batch cross-thread cost.** mimalloc pays at least one locked
  read-modify-write plus a coherence miss on the page header per remote free, and the owner
  later pointer-chases the thread-free list through lines last written on another core.
  tcmalloc pays one lock and a few line transfers per batch. On the reference host an
  uncontended locked instruction costs 20 cycles and a line handed between cores costs
  21 ns within an L3 and 79 ns across (`docs/research/zen5-roofline.md`), against a 2 to 4
  cycle fast path.
  Test: a producer/consumer microbenchmark under `perf c2c`, which should show modified-line
  hits on `mi_page_t` line 1 and `xthread_free`; count locked instructions per free.
- **H2. v3's abandoned-page path.** In a steady-state server most pages have been full at
  some point and are abandoned. Frees into them are a CAS, an ownership claim and
  `mi_free_try_collect_mt` (not inlined; a list walk plus bitmap and shared-counter traffic),
  and a non-originating thread will not reclaim.
  Test: count calls to `mi_free_try_collect_mt` and `_mi_arenas_page_abandon` per million
  frees under a tokio RPC load; rerun with `MIMALLOC_PAGE_FULL_RETAIN=-1` (disables
  abandonment: `theap.c:196`) and `MIMALLOC_PAGE_RECLAIM_ON_FREE=1`. A large swing confirms.
- **H3. Free touches shared metadata.** The Rust `mimalloc` crate calls the unsized
  `mi_free` (section 1.2), and even `mi_free_size` loads `xthread_id` from the page line.
  tcmalloc's sized delete and jemalloc's `sdallocx` compute the class from the size and touch
  only thread-local or CPU-local state.
  Test: L1d and last-level-cache misses attributed to the free entry point, per allocator.
- **H4. Reuse distance.** mimalloc frees go to `local_free` and are not reused until the
  page's `free` list is exhausted, so request-scoped churn cycles through a whole page of
  blocks (64 KiB, against a 48 KiB L1d) instead of reusing the hot block. tcmalloc and
  jemalloc are last-in first-out per class. wasmalloc's roofline saw the same effect as
  churn at twice the floor.
  Test: alloc/free pairs with k live objects; compare L1 miss rate and time per operation.
- **H5. Thread count and migration.** Per-thread heaps scale state with threads (tokio's
  blocking pool, several runtimes) and move with the thread. Per-CPU caches are bounded and
  stay hot on the core.
  Test: pinned against unpinned workers; vary the blocking-pool size; watch RSS and dTLB and
  L2 misses.
- **H6. Back end.** mimalloc purges after 1 s with `MADV_DONTNEED`; tcmalloc keeps hugepages
  intact and releases at a controlled rate.
  Test: page-fault and dTLB-miss counts per request; rerun with `MIMALLOC_PURGE_DELAY=-1`.

Where each stalls or grows [I]. mimalloc: producer pages fill with remotely freed blocks that
are recovered only when the producer's free lists run dry or the page is abandoned and later
reclaimed; a consumer that never allocates that size never reclaims. tcmalloc: consumer
caches overflow and producer caches underflow continuously, serialising on the per-class
transfer-cache lock, and objects recirculate without being re-sorted by address, so spans
stay partially used. The span-prioritisation lists exist to fight this.

## 4. Back ends

- **mimalloc** [S]: arenas of 64 KiB slices, reserved 1 GiB at a time (`options.c:51-53`),
  with all state in atomic bitmaps. Page metadata is at the page start (small pages) or in an
  array at the arena start. Commit is on demand inside a page in steps of 16 KiB or more
  (`types.h:265`, `page.c:843-869`). Fragmentation control is "prefer fuller pages" plus
  abandonment, so other threads can reuse partially free pages. Metadata is roughly 64 to 128
  bytes per 64 KiB page.
- **tcmalloc Temeraire** [S, from its design docs]: its insight is choosing the unit of
  release, the 2 MiB hugepage, and packing so that whole hugepages are either dense or
  empty. Ordering by longest free range avoids spoiling long runs, and counting allocations
  rather than pages predicts emptiness better. Its complexity comes from packing
  variable-length spans into hugepages.
- **Mesh** [S]: a bitmap per span (`mesh/src/mini_heap.h`); per-thread shuffle vectors of
  4-byte (miniheap, bit) entries (`shuffle_vector.h:25-55`); a heap backed by a memfd so two
  spans can be remapped onto one physical page (`meshable_arena.cc:85-143`); randomised
  allocation so spans are likely disjoint; SplitMesher to find pairs. Bitmap spans are worth
  taking on their own merits: frees never write object memory, a double free is a bit test,
  batch frees become word ORs, and emptiness is a popcount. So are compressed 32-bit object
  handles, 16 to a cache line. Meshing itself needs a file-backed heap (which conflicts with
  anonymous huge pages) and randomisation (which conflicts with locality).
- **wasmalloc** [S]: its conclusions that carry over to a native allocator:
  - page kind and header address are pure functions of the `Layout`;
  - alignment up to 4 KiB holds by construction, because the bin size of a multiple of A is
    a multiple of A (proved in `bins`);
  - blocks above the medium limit are header-less runs that grow in place;
  - a single last-in first-out list beat mimalloc's two lists once measured;
  - 16-bit header fields and a cold call on a common transition are expensive;
  - fast paths are `#[inline(always)]`, slow paths `#[cold] #[inline(never)]`;
  - every `unsafe` block has a Kani harness or a soundness-ledger entry;
  - a model-based differential tester and a "floor" allocator in the benchmark harness.

  Its losing workload was `realloc` doubling (a copy at every class change), and its
  footprint cost was one page per touched bin.

## 5. What Rust tells the allocator

Trait facts, from nightly 1.101 `core/src/alloc/mod.rs`. [S]

- `Allocator::allocate(&self, Layout) -> Result<NonNull<[u8]>, AllocError>` (`:232`). The
  returned slice may be longer than the request.
- `deallocate(&self, NonNull<u8>, Layout)` (`:274`). The layout must "fit" (`:143-152`): the
  same alignment as at allocation and any size from the requested size to the returned
  length. The class function must therefore be constant over that interval for a fixed
  alignment. Returning exactly the class size satisfies this when classes are monotone and
  each class size maps to itself.
- `grow`, `grow_zeroed` and `shrink` receive the old and the new layout (`:316,377,444`);
  the alignment may differ between them (`:296`).
- Zero-size allocations are legal for `Allocator` (`:54-56`). `Global` filters them before
  calling `__rust_alloc` (`alloc/src/alloc.rs:331-332`).
- Allocator methods must not unwind (`:175-178`).
- `Allocator` is stable from 1.100
  (`#[stable(feature = "allocator_api", since = "1.100.0")]`, `:196-198`): it is gated on
  stable 1.99.1 and compiles without a gate on nightly 1.101. [S][X]
- `GlobalAllocator: StaticAllocator + Sync + 'static` exists but is unstable
  (`allocator_ext`, `:527-529`). On stable the global allocator is still `GlobalAlloc`: the
  exact layout on `dealloc`, and `realloc(ptr, layout, new_size)` with unchanged alignment.
  `Global` reaches it through `__rust_alloc(size, align)` and friends
  (`alloc/src/alloc.rs:18-43`), so there is no excess-size channel through `Global`.
- `Vec<u8, A>` ignores the excess length. With an allocator that returns 64 bytes for a
  10-byte request, `capacity()` is 10 and `deallocate` receives size 10
  (`bench/probes/vec_excess.rs`). [X] The comments at `alloc/src/raw_vec/mod.rs:495-496,803`
  say the same. So `dealloc` sees the requested size, not the class size.

What Rust provides that C does not, and what each buys:

| Information | Freedom |
|---|---|
| size and alignment on dealloc | The class without any pointer-to-metadata lookup; free touches only local cache state; with a constant layout (`Box`, `Arc`, most nodes) the class is a compile-time constant. |
| alignment on alloc | Alignment by construction (wasmalloc's rule): no over-allocation and no interior pointers. Classes at 8-byte spacing for alignment up to 8 (24, 40 and 56 bytes are common Rust sizes) where C `malloc` must return 16-byte alignment. |
| old and new layout on grow and shrink | In-place decisions without metadata; header-less large runs. |
| zeroed variants | No `memset` for memory that has never been used. |
| returned length | Usable-size feedback for callers that use it. std's `RawVec` does not today. |
| must not unwind or allocate | Fits `#![no_std]` and abort-only slow paths. |

What the C ABI forces us to keep [I]: `free(p)`, `realloc(p, n)` and `malloc_usable_size(p)`
arrive with no size, so some mapping from pointer to class must exist; `malloc` must return
16-byte alignment; `malloc(0)` must return a unique pointer; the `memalign` family is freed
with plain `free`; `calloc`; `errno`; fork handlers; calls before TLS and rseq are ready;
C++ sized and aligned `operator delete` if they are exported. If the class is a function of
the address alone (idea 1 below), the C path costs one shift and one bounds check more than
the Rust path, both share one heap, and C code may free Rust-allocated blocks and the
reverse.

## 6. The kernel's rseq and `mm_cid` on Linux 7.x

These facts decide what a per-CPU front end may assume.

1. **tcmalloc's per-CPU mode does not coexist with glibc's rseq registration.** tcmalloc
   defines its own `__rseq_abi` TLS symbol and registers it with a raw `rseq` syscall
   (`tcmalloc/internal/percpu.cc:86-88`). [S] glibc 2.35 and later register an area for
   every thread first, so tcmalloc's registration should fail with `EBUSY` and leave it in
   per-thread mode. [I] Its own tests expect working rseq only when
   `GLIBC_TUNABLES=glibc.pthread.rseq=0` is set (`tcmalloc/testing/rseq_util.cc:33-46`).
   [S] A tcmalloc baseline on a stock distribution needs that tunable to measure per-CPU
   tcmalloc.
2. **Two rseq modes since v6.19.** [W] A registration of the original 32 bytes gets legacy
   behaviour: IDs rewritten and `rseq_cs` evaluated after every context switch. A
   registration of at least `getauxval(AT_RSEQ_FEATURE_SIZE)` bytes gets the optimized mode:
   IDs written only when they change, `rseq_cs` evaluated only when the thread was
   interrupted and scheduled out or before signal delivery, read-only fields enforced by
   killing the process, and time-slice extension available.
   https://kernel.org/doc/html/v7.2/userspace-api/rseq.html ,
   https://lore.jethome.com/lkml/875x5bzeeb.ffs@tglx/t/
   - tcmalloc's `kernel_rseq` is exactly 32 bytes (`internal/linux_syscall_support.h:28-50`),
     so on its own registration it is in legacy mode. [S][I]
   - glibc 2.43 registers 33 bytes, the full feature size, on the reference host
     (`bench/probes/rseq_auxv.rs`). [X] An allocator that rides glibc's registration is in
     the optimized mode.
   - In that mode, setting the top bit of `cpu_id_start` survives yields and timer
     preemption, and the first migration kills the process with SIGSEGV
     (`bench/probes/rseq_readonly.rs`). [X] The cached-slab-pointer trick is unavailable.
3. **`mm_cid` changed character in the November 2025 rework.** [W] While the number of tasks
   does not exceed the number of allowed CPUs, each task owns its concurrency ID, stable
   across migration. Beyond that the kernel switches to per-CPU ownership and tasks borrow
   the CPU's ID while running. The ID space is `min(nr_tasks, nr_cpus_allowed)`.
   https://lkml.iu.edu/2511.2/08641.html , https://lwn.net/Articles/1042286
   A cache indexed by `mm_cid` therefore behaves as a thread cache when threads do not
   outnumber CPUs and as a bounded CPU cache when they do. Not tested here.
4. **Time-slice extension is not in every 7.x kernel.** The reference host's Fedora build
   has `CONFIG_RSEQ_SLICE_EXTENSION` unset (`/boot/config-7.2.9-201.fc44.x86_64`). [X]

## 7. Ideas considered

Each entry is a mechanism with its status. The adopted ones compose into one architecture:

```
front   per-mm_cid magazines under rseq, proved assembly   one commit store per operation
middle  per-class depot of whole magazines, with heartbeat  a couple of atomics per magazine
back    class regions (the address gives the class), bitmap spans, hugepages by density,
        growable large runs
```

### 1. Class regions: the address is the page map

**Adopted.**

Reserve a large address range split into one region per size class, so
`class = (ptr - HEAP_BASE) >> REGION_SHIFT`. Rust dealloc gets the class from the `Layout`
and C `free(p)` gets it from the address; neither loads metadata. tcmalloc already encodes
three tag bits in the address (`memory_tag.h:28-52`), and Scudo's primary allocator has
per-class regions. [M]

- For: sized and unsized free are both metadata-free; one heap serves both ABIs; a
  mismatched `Layout` is detectable with one compare; every span in a region has the same
  size, so the back end has no external fragmentation between spans.
- Against: physical slack of one partially used span or hugepage per active class; address
  space use (fine at 48 bits); heap layout predictability.

### 2. `mm_cid` magazines over glibc's rseq area

**Adopted.**

Index per-class caches by `rseq.mm_cid`, read at `%fs:__rseq_offset + offset` inside the
critical section: `slab = BASE + (cid << SHIFT)`. There is no cached-pointer trick, no TLS
of our own, no thread-init check and no thread-exit flush. An all-zero header means both
empty and full, so untouched slabs fall to the slow path without an init branch.

- For: tcmalloc's fast-path shape without its ABI abuse; thread-cache locality when not
  oversubscribed; no thread lifecycle code (mimalloc carries scars here: `free.c:482-484`).
  Measured cost over a thread-local magazine: about 2 cycles per inlined pair, nothing out
  of line (`docs/research/zen5-roofline.md`).
- Against: windows where a thread has no rseq registration (very early startup under
  `LD_PRELOAD`, late thread teardown, `CLONE_VM` children such as `vfork` helpers,
  `glibc.pthread.rseq=0`), in which `mm_cid` reads as 0 or stale with no abort protection.
  Remote operations through `membarrier` cost an interrupt. Never write the ID fields.
- Sub-idea, **undecided**: "sticky `rseq_cs`". Put the push and pop paths in one region
  with one descriptor and compare `rseq_cs` against the descriptor address instead of
  storing it, taking the slow path on mismatch. The kernel clears the field lazily, and in
  the optimized mode only after an actual preemption or signal. It trades a store for a load
  and a compare and needs a register convention so the shared abort handler knows what to
  restart; the saving is at most one store per operation.

### 3. Bitmap spans

**Adopted.**

Span free state is a bitmap in dense metadata. Draining a magazine buckets pointers by span,
ORs the indices into a mask per bitmap word, then issues one `lock or` per touched word.
Refill is `tzcnt`/`blsr` over a word, or a vector compress of an index vector, emitting
pointers in address order.

- For: a cross-thread free costs at most one atomic per span word per batch, with no pointer
  chasing at the receiver; freed objects are never written, so clean pages can be dropped,
  there is no free list to corrupt, and a double free is a bit test at drain; refilled
  magazines are address-sequential from one span, which restores the spatial locality
  tcmalloc's recirculating transfer cache loses; emptiness is a popcount.
- Against: metadata of one bit per object plus 8 bytes per cached object; a multiply on the
  drain path; contended `lock or` on one hot span.

### 4. Magazine depot with pointer-swap exchange and a heartbeat

**Adopted.**

Per (cid, class), keep a current magazine and exchange whole magazines with a per-class
depot instead of copying batches. Producer/consumer steady state costs a couple of atomics
per magazine and no copying. Every exchange is a heartbeat, deterministic in the number of
operations, that runs a bounded quantum of work: return one depot magazine to its spans,
advance decay and release, account sampling for the batch.

- For: tcmalloc's transfer cache without the lock or the copy; mimalloc's pacing without
  per-page lists; the rate of returning magazines to spans is one knob trading speed for
  packing.
- Against: ABA in a lock-free depot; magazine size tuning per class; memory parked in
  magazines (bounded by concurrency IDs, not threads).

### 5. Proved assembly kernels

**Adopted.**

Write the fast-path sequences in assembly, chosen by measurement on the host, and prove the
winner: functional correctness against an abstract stack; restartability (an abort at any
instruction boundary, with arbitrary interference on that ID's slab, is equivalent to not
having started); memory safety for any ID value; the clobber contract. Assembly also pins
choices a compiler would undo: 64-bit state words only, base-register addressing, and code
layout (`docs/research/zen5-roofline.md`).

- Against: code size if inlined at every site (wasmalloc saw 1.46 MB over 18,000 sites in
  one application); an `asm!` block is opaque to LLVM, which defeats some reasoning such as
  allocation elision.
- Sub-idea, **undecided**: per-class entry thunks with the class as immediates and a private
  minimal-clobber convention, called from an `asm!` block with an explicit clobber list, so
  callers keep values in registers across an allocation.

### 6. Growable large runs

**Adopted.**

Large allocations are header-less runs placed with address-space headroom, so `grow` extends
in place by touching more pages, with `mremap` as the fallback. The size comes from the
`Layout`.

- For: `Vec`, `String` and `BytesMut` doubling becomes O(1) with no copy, the workload
  wasmalloc lost on; no metadata line touched for large frees from Rust.
- Against: address-space budget and overcommit accounting; RSS if slots are recycled without
  decay; page-fault cost on growth.

### 7. Hugepage-by-density back end

**Adopted.**

With same-size spans per class region, Temeraire's bin-packing problem disappears: any free
span fits any request. Allocate lowest address first; back dense 2 MiB chunks with huge
pages (tcmalloc already calls `MADV_COLLAPSE` with backoff:
`internal/system_allocator.h:58-59,743`, `huge_page_filler.h:668` [S]); sub-release sparse
chunks; track which memory has never been used so `alloc_zeroed` skips the `memset`.

- For: Temeraire's hugepage coverage with a fraction of the machinery; release decisions are
  per-chunk occupancy.
- Against: slack in sparse classes; interactions with khugepaged; `MADV_COLLAPSE` latency.
- 64 KiB multi-size huge pages would suit sparse chunks, but they are a system-wide setting
  that is off by default on the reference host, so the design cannot depend on them.

### 8. Slice-extension micro-locks

**Later.** Unavailable on the reference host (section 6, item 4).

Kernel 7.0 and later let a thread request a short scheduler grace period by setting
`rseq->slice_ctrl.request`, enabled per thread by `prctl`. [W] Used around depot and
span-list critical sections, plain test-and-set locks would not suffer lock-holder
preemption, and slow paths could be as simple as spinlocked code. The grant is best-effort,
so correctness cannot depend on it.

### 9. Compressed 32-bit handles in magazines

**Later.**

Store `(ptr - HEAP_BASE) >> 4`, or a per-class index, instead of pointers: 16 handles per
cache line. It halves the hot footprint of the front end at the cost of one more instruction
on the result path and a reach limit per handle.

### 10. Meshing

**Not adopted.**

Mesh compacts without relocation by remapping two spans with disjoint live sets onto one
physical page. A narrowed form was considered: deterministic rotation offsets instead of
random allocation, applied only to cold sparse spans under memory pressure. It is out of
scope. Real workloads have not been observed to need it when classes are segregated and the
allocator is careful about reusing partially full memory before fresh pages, and it would
require file-backed memory, which gives up anonymous huge pages. Bitmap spans are adopted
for the reasons in idea 3, not to keep meshing open.

### 11. Origin-affine return

**Later.**

Because a span's owner shard (NUMA node or L3 group) is a function of its address, objects
returned from magazines go back to their origin span's bitmap rather than to whoever freed
them. Memory allocated on node A and freed on node B returns to A. It only matters on hosts
with several L3 domains or NUMA nodes; the reference host has two L3 domains.

## 8. Not verified

- How `tikv-jemallocator` and the tcmalloc wrapper crates call into C (sized or unsized
  free). [M] The `mimalloc` crate was checked (section 1.2).
- That tcmalloc and jemalloc beat mimalloc on tokio-style workloads: not measured here, and
  the hypotheses in section 3 are untested.
- That tcmalloc's rseq registration actually fails under glibc 2.43; this is read from
  source, not run.
- What `cpu_id` and `mm_cid` read as outside the registered window (early startup, thread
  teardown, `vfork` children), and the `mm_cid` ownership behaviour of section 6.
- `MADV_COLLAPSE` behaviour under load and vector-compress cost on Zen 5.
- jemalloc and Scudo details, which are from memory.
- tcmalloc's `huge_page_filler.h`, `huge_region.h`, `span.h` and the x86-64 `.S` batch
  routines were not read line by line; back-end statements about tcmalloc come from
  `docs/temeraire.md` and `docs/design.md` in its tree.
- Trait text was read from nightly only; the stabilisation state was checked by compiling.
