# Huge pages for large blocks allocated whole: the bound

2026-10-24, main `46126f0`, on the reference host (Ryzen 9950X, Linux 7.2.9, transparent huge
pages `madvise`, defrag `madvise`, khugepaged `max_ptes_none` 511). Measured with a run-time
switch in one binary; the switch is a probe, not product code
(`bench/probes/whole-block-huge/`).

## What matters

- **No server gains.** On every server job the bound is within run-to-run noise (under 1%),
  and on three of the four the switch never acts: the log store, the 2 GiB order service and
  the HTTP/2 RPC job allocate no block of 2 MiB or more whole. Their dTLB walks are in small
  classes, and on the order service in its 1.25 MiB table shards, which no per-block huge page
  can cover. Meilisearch has the blocks, but only about 3% of its walks land in them.
- **Two batch programs gain about 2 to 3%, and the HashMap kernel 10%.** DataFusion is 2.9%
  faster (resolved: 68.1 to 68.2 against 70.0 to 70.3 Gcycles), with half the page faults and
  half the walks and the same peak. tpchgen is 2.0% faster over nine rounds, with overlapping
  ranges. The HashMap job is 10% faster, 2% behind mimalloc where it was 15%.
- **The cost of being wrong is large and has no ceiling but the block.** A block touched in
  one page in 64 holds 11 to 16 times its memory. A block written in a short prefix holds
  its first huge page or more. And on a host short of free 2 MiB blocks, every advised fault
  can stall in direct compaction: in one session lives slowed by up to 17 times.
- **Recommendation: a documented limit, not a mechanism, for now.** The gain is on batch
  programs and the kernel job, not on the servers that lead what we optimize. If a mechanism
  is wanted later, the cheapest evidence is the request's own shape (not zeroed) gated by
  what the free of the same class saw; a look by the background thread races with the
  holder's free and needs a new protocol (ranked at the end).

## Method

- **The switch** (`wbh_edit.py`, `wbh.rs`) is applied to a scratch copy of main by
  `build.sh`. `ASMALLOC_WBH=m<mode>f<MiB>d<0|1>` is read once. A large block allocated whole
  (a plain allocation, not the destination of a growth with headroom) of at least the floor
  gets the grown path's mark before it is cleared or touched: its slot's whole huge pages up
  to its size advised for huge pages and `Recorded::huge` set. Its free then takes the grown
  path: kept with its advice within the kept-grown budget, else returned whole with the
  advice taken back. Arms: `main` (m0), `all2` (every whole block of 2 MiB or more), `nz2`
  (the same, requests that are not zeroed), `all16` (every one of 16 MiB or more). Every arm
  is one binary with an environment of equal length.
- **Counters.** Zen 5's `ls_l1_d_tlb_miss` (0x45): every L1 dTLB miss (0xff45), and the L2
  misses that start a page walk for 4 KiB pages (0x1045) and 2 MiB pages (0x4045), user mode
  only. Kernel jobs count them in their phases (`BENCH_EVENTS`); the preloaded programs under
  `perf stat`; the servers in benchrun's per-group counters, through a scratch runner
  (`runner_edit.py`, `WBH_PROC_EVENTS`). DataFusion has room for one raw event beside the
  runner's, so it counts all walks (0xf045) only.
- **Huge pages the process got.** Every run records its peak and mean `AnonHugePages` (the
  runner's rollups; the drivers sample `smaps_rollup` every 100 to 200 ms), the machine's
  `thp_fault_alloc`, `thp_fault_fallback` and `compact_stall` over the run, and the free
  blocks of 2 MiB or more in `/proc/buddyinfo` before it. No server or app run saw a THP
  fallback or a compaction stall. Free 2 MiB blocks ranged from 130 to 3,400 over the day.
- **Attribution.** IBS op samples (`ibs_op`, user mode, with data addresses), sorted by the
  asmalloc region they fall in, from a map the switch writes as regions are assigned
  (`ibs.py`, `sessions/attrib*.sh`). A walk is a sample IBS marks as an L2 TLB miss. These
  runs are separate from the timed ones; walk samples per job: 11 (jaq) to 3,010
  (Meilisearch).
- **Runs.** HashMap: `kern.py`, CPU 2, ten rounds over two sessions. tpchgen, jaq,
  rust-analyzer: `pre.py` with the preload library, three rounds (tpchgen nine). DataFusion:
  four rounds. Servers: one round each (Meilisearch's main two), under `scripts/measure
  --both-l3`. A single server run in one binary moves by up to 2% between runs: on the log
  store, where the switch never acts, `all2` read 1.8% under `main`.
- Raw data: `docs/research/data/whole-block-huge-2026-10-24.tar.gz`.

## 1. Where the walks are

4 KiB page walks, asmalloc on main against the allocators that get huge pages; and, for
asmalloc, the share of walks by where the address lies (IBS).

| job | walks: asmalloc / mimalloc / tcmalloc | asmalloc's walks: small / large <2M / large 2-16M / large ≥16M / outside the heap |
|---|---|---|
| HashMap 1M (a run) | 2.63M / 0 / 7.19M | 17% / 1% / 11% / 69% / 2% |
| DataFusion (a run, all page sizes) | 56M / 2.7M / 47M | 18% / 15% / 12% / 45% / 10% |
| tpchgen (a run) | 17.4M / 8.6M / 14.0M | 10% / 15% / 0% / 71% / 4% |
| rust-analyzer (a run) | 2.7M / 1.3M / 3.7M | 34% / 41% / 3% / 0% / 22% |
| jaq (a run) | 2.3M / 2.2M / 3.9M | 55% / 0% / 0% / 0% / 45% (11 samples) |
| Meilisearch (a request) | 1,763 / 534 / 1,712 | 57% / 1% / 2% / 1% / 39% |
| log store 1M (a request) | 28 / 0.4 / 82 | 92% / 3% / 0% / 0% / 5% |
| order service 2G (a request) | 3.8 / 0.1 / 6.6 | 23% / 70% / 0% / 0% / 7% |
| gRPC log h2 (a request) | 0.27 / 0.09 / 5.0 | not sampled |

- **tcmalloc gets no huge pages on this host**: zero `AnonHugePages` and THP faults in
  every run, preloaded or linked. Only mimalloc does.
- **Where it can matter**, the walks are on large blocks of 2 MiB or more: HashMap,
  DataFusion, tpchgen. On the servers they are not. Meilisearch's "outside the heap" is its
  memory-mapped store and glibc's arenas for its C code; its small classes already have huge
  pages past their first 16 MiB.
- **The order service's 70%** is its presized table, sharded into 1.25 MiB blocks in one
  class region. No block of it holds a whole huge page; only a region-level mark, as small
  classes get, could cover it. At 3.8 walks a request it is worth at most a few hundred
  cycles of 67k.

## 2. The bound: the switch's arms

Medians; cycles against main in the same session. Memory in MiB: the cgroup peak, or for
servers the anonymous memory's mean and peak at 70% load.

| job | main | all2 | nz2 | all16 | mimalloc | tcmalloc |
|---|---|---|---|---|---|---|
| HashMap: Mcycles | 2,813 | -10.5% | -11.6% | -9.2% | -12.5% | +49.5% |
| faults / 4K walks / peak | 53k / 2.63M / 205 | 21k / 0.22M / 180 | 21k / 0.22M / 180 | 25k / 0.30M / 188 | 0.2k / 0 / 299 | 122k / 7.2M / 254 |
| DataFusion: Gcycles | 70.15 | -2.9% | -1.8% | -2.4% | -0.4% | -3.7% |
| faults / walks / peak | 1,035k / 56M / 864 | 503k / 29M / 865 | 647k / 30M / 862 | 594k / 32M / 865 | 123k / 2.7M / 1,693 | 449k / 47M / 999 |
| tpchgen: Gcycles (9 rounds) | 26.20 | -2.0% | -1.9% | -2.3% | -4.8% | -4.4% |
| faults / 4K walks / RSS peak | 708k / 17.4M / 557 | 620k / 7.9M / 553 | 623k / 7.9M / 541 | 620k / 8.0M / 541 | 78k / 8.6M / 779 | 156k / 14.0M / 526 |
| rust-analyzer: Gcycles | 24.44 | -1.3% | -1.2% | -0.5% | +2.2% | +3.5% |
| jaq: Gcycles | 13.52 | level [1] | level | level | level | level |
| Meilisearch: Mcycles a request | 16.97 | -1.1% | -0.7% | +0.3% | +8.2% | +2.2% |
| faults a request / heap mean / peak | 53 / 725 / 769 | 40 / 745 / 798 | 45 / 731 / 776 | 52 / 738 / 779 | 25 / 1,605 / 1,750 | 0.1 / 972 / 975 |
| order service 2G: kcycles a request | 66.8 | -0.1% (does not act) | | | +6.2% | +3.0% |
| log store 1M: kcycles a request | 357 | -1.8% (does not act) | | | +22.8% | +13.7% |
| gRPC log h2: kcycles a request | 127.7 | +1.2% | -0.1% | +0.5% | +3.0% | +6.0% |

[1] jaq's time moves with the machine's free huge pages (its kernel time went from 5.4 to
4.2 Gcycles across the session for every arm), not with the switch; it allocates no large
block that matters.

- **What the arms got.** HashMap: 308 THP faults a run, 72 MiB of huge pages at the
  sampled peak. DataFusion: about 1,130 THP faults a run (2.2 GiB), but at most 54 MiB at
  once: its large blocks are short-lived, and each free returns its slot. tpchgen: 336 MiB.
  Meilisearch `all2`: 5,925 THP faults (11.6 GiB in a run, its 50 MiB zeroed buffers and the
  indexer's blocks), 102 MiB on average and 712 at the peak.
- **Where DataFusion's 2.9% comes from**: about half from faults (532k fewer, the large
  blocks that the hold returns and the program takes again now fault 2 MiB at a time) and
  half from walks (27M fewer). Its peak does not move.
- **On Meilisearch the crude arm is a small cost, not a ruin**, because every zeroed buffer
  that is now advised is also returned whole at its free, as the evidence rule already
  returns them on main. `all2` holds 20 MiB more on average and 29 MiB more at the peak;
  `nz2` 6 and 7. The one-round cycle differences are inside the 0.4M spread between rounds
  of one build (`meili-peak.md`).

## 3. The cost of being wrong

`sparse/`: blocks allocated whole, held, freed, six lives (the first left out), CPU 2,
three rounds; kcycles a block's life and MiB held for all blocks at once. Session `sp2`, with
3,200 free 2 MiB blocks and no compaction stall.

| case | main | all2 | nz2 | all16 |
|---|---|---|---|---|
| 4 x 64 MiB, one store in 1/64 of the pages at random | 631 kcyc, 16 MiB | 12,373, 256 | 12,269, 256 | 12,491, 256 |
| the same, 1/16 of the pages | 2,168, 57 | 12,508, 256 | 12,349, 256 | 12,422, 256 |
| the same, zeroed (`alloc_zeroed`) | 410, 4 | 12,427, 256 | 416, 4 | 12,343, 256 |
| 8 x 16 MiB, 1/64 of the pages | 142, 8 | 1,991, 128 | 1,841, 128 | 1,832, 128 |
| 4 x 64 MiB, a 1 MiB prefix written | 29, 4 | 96, 8 | 96, 8 | 95, 8 |
| 8 x 16 MiB, a 256 KiB prefix | 8, 2 | 65, 16 | 66, 16 | 65, 16 |
| 4 x 64 MiB written whole, then random reads | 506, 256; 64 cycles a read | 12,390, 256; 63 | 12,291, 256; 62 | 12,421, 256; 63 |

- **Memory.** A sparse block costs every huge page it touches: 16 times main at 1/64, 4.5
  times at 1/16. A prefix costs its first huge page at least (2 MiB for 256 KiB).
- **Time.** Most of the extra time is not the TLB but the free: an advised block's slot is
  returned at its free, as the grown path's is, so the next life faults and clears 64 MiB
  again, where main's resident slot costs nothing. A block written whole and reused pays 25
  times main for each life. Random reads over 256 MiB were 1 to 3% faster, because each one
  misses to DRAM anyway.
- **A host short of huge pages.** In the first session (`sp1`) free 2 MiB blocks fell from
  280 to 130 while other work ran. The arms' lives then took up to 170M cycles a 64 MiB block
  (17 times their usual), got only part of the block in huge pages, and took up to 50k small
  faults: defrag `madvise` makes an advised fault compact memory synchronously. Main, whose
  regions are advised against huge pages, never pays it. mimalloc's lead in `grown-slots.md`
  vanished for the same reason.
- **khugepaged** collapses any advised range with one page present (`max_ptes_none` 511), so
  a sparse block that faulted in small pages under pressure is filled in later anyway.

## 4. Evidence that could separate dense from sparse, and what each costs

The bound passes the 2% bar on two batch programs, so the options are set out; none is
built.

- **`MADV_COLLAPSE` on our regions** (`collapse.c`, the kernel of this host). On a range
  advised `MADV_NOHUGEPAGE`, as every region starts, it fails with `EINVAL`. After
  `MADV_HUGEPAGE` it works: about 220 us for each dense 2 MiB (it copies). On a sparse range
  it ignores `max_ptes_none` and fills every page: 32 KiB touched in 16 MiB became 16 MiB
  resident. So a collapse must follow a decision that the block is dense, and needs an
  advice change first, which takes the mapping lock for writing and splits the mapping.
- **The request's own shape (`nz2`).** Free. It keeps nearly all of the gain (HashMap -11.6%,
  tpchgen -1.9%, DataFusion -1.8%) and spares Meilisearch's zeroed buffers. It does nothing
  for a sparse block that is not zeroed: a `Vec::with_capacity` or a hash table sized far
  beyond its use pays the full cost above.
- **Learning at the free.** At the free of an unadvised whole block of the floor or more, ask
  `mincore` about sixteen pages, as the free of an unwritten block does today (about 12k
  cycles), and keep a hint word for the class: dense if fifteen of sixteen are resident. The
  next whole block of the class gets the mark while the hint holds. Costs: the sample, at most
  2% of a 2 MiB block's life that faults its pages, but up to all of it for a resident slot
  barely touched, so it should sample only some frees. Limits: an advised block reads as
  dense to `mincore` whatever its tenant did (each touch makes 2 MiB resident), so the hint
  must be checked on unadvised lives, say one in sixteen, which give up the gain; a class
  shared by dense and sparse tenants flips; and it never helps the first block of a class,
  which is every table of a HashMap growing once. The hint word is written by any freeing
  thread and read by any claiming one, which is safe for a hint: a wrong value costs memory or
  time, never a slot.
- **A look by the background thread at live blocks**, `mincore` at sixteen points, then the
  mark or `MADV_COLLAPSE`. It finds the work where it is (the program's threads pay nothing,
  and compaction lands on the background thread), and it can see one-shot blocks. But a large
  slot's size word has one writer, its holder, and the look acts on a slot it does not hold:
  - The holder can free the block between the look and the action. If the background then
    advises the range, the advice outlives the tenant: the size word says `huge` is clear, so
    no free or claim takes it back, and the next tenant gets huge pages unmarked. If it
    collapses, the kernel makes resident every page of a slot whose clean bits say none are,
    so neither the retained count nor the purge sees that memory until a later tenant frees
    the slot. Contents stay correct either way: collapse copies what is there, and zero pages
    are zero.
  - Setting `huge` from the background makes the size word multi-writer. A free that reads
    the word just before the background's write takes the unadvised path and leaves the
    advice behind; one just after takes the advice back while the background's `madvise` is
    still to come. Closing both needs a handoff: the background claims the word with a
    compare-and-swap of a busy bit, a free that finds it busy marks the slot freed and leaves
    the release to the background, which finishes, takes the advice back and releases. That
    is a new protocol on the large free path (a compare-and-swap where a load serves today, a
    model in `tla/`, ledger entries for the new system calls).
  - It needs the background thread, which a program without it, or before it starts, lacks.

## Surprising

- tcmalloc gets no huge pages at all here, so the comparison is with mimalloc alone.
- The servers' walks are small-class walks. The order service's are in 1.25 MiB shards, a
  case for a region-level mark of a dense large class, not a per-block one.
- Most of the arms' cost on reused blocks is the grown path's free (return the slot whole),
  not the huge pages: main's resident slot is reused for nothing.
- Random reads over a large resident set barely gain: the TLB walk overlaps the DRAM miss.

## Unresolved

- A variant whose free keeps an advised whole block resident, as a plain free does, was not
  measured. It would remove most of the reuse cost above and might raise DataFusion's peak.
- tpchgen's 2.0%: nine rounds, ranges overlap (25.59 to 26.23 against 25.87 to 26.74).
- The servers ran one round per arm; their bound is set by the attribution and the counts,
  not by those single runs.
- A region-level mark for dense large classes below a huge page (the order service's
  shards), and whether it would move anything at 3.8 walks a request.

## Recommendation, ranked

1. **A documented limit.** Large blocks allocated whole get no huge pages; it costs a dense
   batch program 2 to 3% (DataFusion, tpchgen) and a HashMap-building kernel 10%, and no
   server measured anything. Suggested design text, after "A large block allocated at its
   size in one call gets no huge pages" in "Returning memory and huge pages": *That costs a
   program that fills such blocks densely, a query engine's hash tables, 2 to 3% of its time
   and a HashMap built to a million entries 10%, and the servers measured nothing: their
   walks are on small classes (`docs/research/whole-block-huge.md`).*
2. **If a mechanism is wanted:** the shape (not zeroed, 2 MiB or more) gated by a per-class
   hint learned at the free of unadvised blocks, with the advised block's free keeping its
   slot resident. No new protocol; the risk is a sparse block that is not zeroed in a class
   the hint calls dense. Measure the resident-free variant first.
3. **Not now:** the background look. It is the only option that sees one-shot blocks, and it
   costs a protocol on the large free path for a gain no server shows.
