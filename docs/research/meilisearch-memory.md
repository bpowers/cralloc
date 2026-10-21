# Meilisearch's memory and the hold

2026-10-21, main `ff75381`, with the large-heap suite's `meilisearch hn` job at 930 req/s
(`servers-meilisearch.md`). One finding per question:

- **The swing and the doubled faults are the hold.** With the hold off, asmalloc's faults per
  request fall from 71 to 88 to 34 to 39, jemalloc's level (38 to 42). The heap's swing from
  one ten-second piece to the next falls from 50 to 170 MiB to 15 to 40, and it settles at about
  950 MiB. Nothing else returns that memory under load.
- **No hold length serves both programs.** DataFusion needs the 125 ms hold to keep its peak
  under jemalloc's. Meilisearch faults back what the hold returns 30 ms to 2 s later, so only a
  hold of about half a second or more stops its faults. At half a second DataFusion's peak goes
  back to where it is with no hold, 250 to 380 MiB higher, not 30. The hold must adapt.
- **Most of the "heap" is not the allocator's.** About 560 to 590 MiB of Meilisearch's
  anonymous memory, the same under either allocator, is in glibc malloc's arenas: 64 MiB-aligned
  heaps that C code fills (LMDB is the likely user). Of the rest, at rest, asmalloc holds 285
  MiB, 228 of it free whole pages of large classes that nothing returned, against about 180 MiB
  that jemalloc holds (active plus dirty).

## The hold's length

Meilisearch, two rounds each (`meilisum.py`; heap is anonymous memory at the end of each
ten-second piece of the minute at 70%, and 9.75 s after the load stops):

| hold | faults/req | heap at 70%, MiB | swing | at rest |
|---|---:|---:|---:|---:|
| off | 34, 39, 39 | 927 to 968 | 14 to 41 | 934 to 952 |
| 64 steps (125 ms, main) | 71 to 88 | 739 to 935 | 49 to 172 | 826 to 891 |
| 256 steps (0.5 s) | 40, 43 | 899 to 972 | 57, 60 | 938, 943 |
| 1024 steps (2 s) | 36, 37 | 942 to 990 | 9, 27 | 943, 966 |
| jemalloc | 38 | 743 to 774 | 31 | 748 |

Cycles per request moved with the round (15.3M to 17.2M), not with the hold; p99.9 was 43 to
50 ms for every hold.

DataFusion SF3, five rounds, cgroup peak:

| hold | peak MiB |
|---|---:|
| 64 steps | 843 to 857 |
| 256 steps | 1,095 to 1,233 |
| 1024 steps | 1,053 to 1,244 |
| jemalloc | 854 to 872 |

**What Meilisearch faults back, and how long after the hold returned it.** A claim of a large
slot that finds pages of it clean (returned) faults them back. With the hold at 64 steps, over
one run (`edits/revival.py`): the bytes so faulted back, by the time since the hold last returned
memory in that region, were 3.6 GB at 64 to 127 ms, 2.0 at 128 to 255 ms, 2.9 at 0.26 to 0.5 s,
3.6 at 0.5 to 1 s and 3.7 at 1 to 2 s. Less came back after longer or shorter times. By class,
the faulted-back bytes are blocks of 8 to 32 MiB (17 GB), 64 to 128 KiB (4.0 GB) and 4 to 8
MiB (1.2 GB): the indexer's buffers, allocated for each batch of updates, freed, and allocated
again at the next batch, a fraction of a second to two seconds later. DataFusion's large slots
come back within 60 ms or not at all (`memory.md`, "Faults and system calls in real programs").

So a class's evidence decides its hold: one whose returned slots are taken again within a
couple of seconds should hold for longer than its gap, and one whose slots are not taken again
should go back at the short hold. The adaptive hold measured on DataFusion alone (doubling on a
claim within four holds of the hold's purge, halving after four holds without one) held the
peak level for 9% more faults (`large-hold.md`). Against these two programs it should be judged
first on DataFusion's peak and Meilisearch's faults, and only then on DataFusion's faults.

## The account

Under load (memstat at 120 s, the sample where the program held the most) and at rest (at
exit, 10 s after the load stopped): asmalloc's sampler (`memstat.rs`, every 30 s) and the
whole-process account at exit (`memacct.rs`, `acct.py`). Both of asmalloc's account runs were
marked invalid: the load generator reported 15k errors against Meilisearch's update tasks
(`Can't delete file`) with the sampler's build only, so their load differs. The state at rest
comes mostly from the start's indexing, before the load.

| MiB | asmalloc under load | asmalloc at rest | jemalloc at rest |
|---|---:|---:|---:|
| anonymous memory | | 868 | 644 |
| glibc arenas (C code), and other maps outside the allocator | | about 590 | about 560 |
| the allocator's objects, live | 291 | 43 | (active 100) |
| free in bitmaps on wholly free pages: large classes | 40 | 219 | (dirty 78) |
| free in bitmaps on wholly free pages: small classes | 5 | 9 | |
| free slots on pages pinned by live objects | 3 | 6 | |
| in magazines and the depot | 7 | 8 | |
| metadata and the magazine area | 2 | 2 | |

- At rest, asmalloc's extra over jemalloc is free whole pages of large classes that nobody
  returned. That is 188 MiB in slots of the 56 MiB class, freed as the load ended, plus 17 MiB
  of the 1.25 MiB class. It is not fragmentation: small-class free slots pinned by live objects
  are 6 MiB. Nothing runs maintenance at rest in this process (no heartbeats once the load
  stops, and the default thread is not merged), so the hold and the clocked pass never see them.
  With heartbeats or `maintain` they are over the floor and would go.
- Under load, asmalloc's part is about 360 MiB with 290 live. jemalloc's statistics, from
  tikv-jemallocator's build without statistics, give only its pages: 100 to 350 MiB active and
  44 to 78 MiB dirty.
- The heap the suite reports (`servers-meilisearch.md`) adds the glibc arenas to each
  allocator's part. The difference between allocators is in the 300 or so MiB beside them.

## Method

Builds of main with `bench/probes/app-faults/edits`: `nohold.py` and `holdsteps.py` for the
hold's length, `revival.py` for what is faulted back, `jestat_all.py` for jemalloc's
statistics through a run; `appvariant.sh` for each, run through `benchrun` with local `build`
entries under `scripts/measure --both-l3`.
