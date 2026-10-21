# Meilisearch: a search engine with a large heap

Measured 2026-10-21 on branch `apps-meilisearch` (allocator code as at `d945f62`), with the
large-heap suite (`bench/apps/README.md`, "A search engine"): Meilisearch 1.55.0 on CPUs 0-3,
its cores held out of deep idle, over 400k Hacker News items it indexes as it starts; the load
generator on 8-11, 128 connections, searches for one to three title words with a batch of 50
updated items every 200th request. The large-heap day at 930 req/s (70% of mimalloc's
saturated 1,328; asmalloc saturates at 1,413, jemalloc 1,372), three rounds as separate
sessions. No run invalid, none with other work on the server's CPUs. tcmalloc was left out of
these, and measured in a second session once its wrapper was fixed ("tcmalloc", below). Raw
results of both: `docs/research/data/servers-meilisearch-2026-10-21.jsonl.gz`.

## Heap and mapped store

Of Meilisearch's 3.5 to 4.6 GiB resident under load, about 2.8 GiB is its LMDB store, mapped
from a file and the same for every allocator; the rest, anonymous memory, is the heap, which is
the allocator's part. The table below is about the heap only.

The program's own live bytes (the counting build, read at each mark) alternate between about
60 MiB and 270 MiB: the larger when a batch of updates is being indexed. They are 50 MiB
when the server starts to listen, after indexing the 400k items.

## Results

Medians of three. Cycles per request and p99.9 at 70%; heap is the anonymous memory, at the
end of the ten-second pieces of the minute at 70% (lowest to highest) and ten seconds after
the connections closed:

| Allocator | cycles/req | p99.9 ms | faults/req | heap at 70%, MiB | heap idle, MiB | in huge pages, MiB |
|---|---|---|---|---|---|---|
| asmalloc | 17.1M | 45 | 99 | 704 to 931 | 893 | 38 |
| jemalloc | 17.7M | 56 | 42 | 740 to 774 | 747 | 0 |
| mimalloc | 18.3M | 60 | 27 | 1,495 to 1,900 | 1,700 | about 1,400 |

Set against the live count at the same marks (`largesum.py --heap`), that is 3.3 to 3.5 times
live for asmalloc, 2.8 for jemalloc and 6.4 to 6.8 for mimalloc under load, and 16, 14 and 31
times at rest. No run had a direct compaction.

Memory system calls, per 1,000 requests (`run.py syscalls`, strace -c over 20 s at 930 req/s,
outside the timed runs): madvise asmalloc 684, jemalloc 688, mimalloc 194; mprotect 176, 457
and 378; mmap and munmap 16 for each (Meilisearch's own threads and maps).

## What it shows

- **asmalloc does the least work per request**, 3% under jemalloc and 6% under mimalloc, and
  has the best tail (45 ms against 56 and 60). A request costs millions of cycles because the
  indexing of the updates is spread over the searches.
- **Every allocator keeps the memory of the start's indexing.** Ten seconds after the load
  stops, the program holds 50 MiB and the allocators 0.75 to 1.7 GiB. asmalloc keeps about
  140 MiB more than jemalloc.
- **asmalloc's heap breathes and the others' do not.** It moves between 0.7 and 0.93 GiB from
  one piece of the minute to the next, as batches are indexed and freed, where jemalloc stays
  within 35 MiB. It returns memory between batches and takes it back at the next, which
  presumably makes most of its 2.4 times jemalloc's page faults, with about as many madvise calls as
  jemalloc.
- **mimalloc puts the heap in huge pages** (1.4 of its 1.7 GiB), and holds twice the others.
  asmalloc has 38 MiB in huge pages: its classes over 1 KiB get none, and Meilisearch's
  indexing buffers are large.

## tcmalloc

tcmalloc aborted in every start, while the server indexed: "Mismatched-size-delete of 8 bytes
(expected 2 bytes)", for a sampled allocation made in charabia's `LowercaseNormalizer` and
freed in milli's `DocumentTokenizer::tokenize_document`. The program was not at fault. The
suite's tcmalloc wrapper (`bench/alloc/src/tc.rs`) kept a block in place when `realloc` grew it
within its size class, and tcmalloc checks a sampled block's sized free against the size first
requested: a 2-byte string grown to 8 in place and freed as 8 fails that check. A throwaway
build whose `realloc` always copies started cleanly three times out of three, and the wrapper as
it was aborted three times out of three. The wrapper now copies on every `realloc`, as the
`tcmalloc-better` crate it follows does. With it, tcmalloc's check (on sampled blocks only)
found no free whose size differed from the allocation's in three starts and nine runs, so
there is no sign of one that could put a block in the wrong asmalloc class.

A second session, three rounds with all four allocators (tcmalloc saturates at 1,423 req/s, so
the rate stays 930):

| Allocator | cycles/req | p99.9 ms | faults/req | heap at 70%, MiB | heap idle, MiB | resident over live |
|---|---|---|---|---|---|---|
| asmalloc | 17.1M | 44 | 88 | 691 to 945 | 737 to 905 | 3.4 |
| tcmalloc per-CPU | 17.3M | 48 | 0.1 | 935 to 985 | 929 to 939 | 3.6 |
| jemalloc | 17.7M | 52 | 42 | 723 to 772 | 735 to 758 | 2.8 |
| mimalloc | 18.3M | 54 | 27 | 1,470 to 1,673 | 1,492 to 1,680 | 5.9 |

The first session's order and margins hold. tcmalloc faults almost never: it keeps what it
took and returns none of it, so its heap is steady at about 0.95 GiB.

## Method notes

- The heap is the anonymous memory from `/proc/<pid>/smaps_rollup`, read once a second, now
  the third column of a server run's curve. Meilisearch's indexing writes its temporary data
  to files, not anonymous maps, so the anonymous memory is the heap and thread stacks.
- The store is written fresh each run, in a directory under the inputs on disk; its file pages
  count toward the cgroup's peak (3.6 to 4.8 GiB) but not toward the heap.
- The live count is read only at the plan's marks, ten seconds apart at 70%, so the peak of
  the start's indexing, before the first mark, is not measured.
