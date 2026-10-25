# Heap drift over long runs at large heaps

2026-10-24 and 25, allocator code at main `5536610`, on the reference host (Ryzen 9950X,
Linux 7.2.9, transparent huge pages on `madvise`). Every earlier memory measurement was
minutes long at a steady mix. This note runs time-compressed stand-ins for hours of service,
billions of operations each at 1 to 2 GiB live, and asks whether asmalloc's resident memory
creeps up against what the program holds, compared with jemalloc, tcmalloc (per-CPU),
mimalloc and glibc, and where the memory is when it does.

## Summary

- **At a steady size mix asmalloc does not drift, and holds the least.** A cache of 2 GiB
  churned for 15 billion operations ends at 1.105 times live, flat from the first minute,
  against tcmalloc 1.127, jemalloc 1.148, glibc 1.27 and mimalloc 2.97. With the number of
  busy threads swinging between 8 and 1, asmalloc settles at 1.22 and is again the lowest
  (tcmalloc 1.38 and still creeping, jemalloc 1.2 to 2.0, mimalloc 2.0, glibc 2.3).
- **When the size mix wanders, asmalloc ratchets.** Each size class keeps resident about
  1.1 times the most the program ever asked of it. A lifetime mix whose per-class demand
  moves by 20 to 30% between load peaks climbs from 1.10 to about 1.21 over 24 billion
  operations, then holds there for 48 billion more. On the same program jemalloc stays at
  1.14 to 1.16 and glibc at 1.075. With the class mix held fixed, asmalloc is flat at 1.107.
  The ratchet is bounded: it costs about 6 points over jemalloc at steady state, not
  unbounded growth.
- **The memory is free slots in 2 MiB huge-page units that still hold a few objects.** At the
  end of a late peak the 212 MiB of growth is all free memory the purge cannot return, 176 MiB
  of it whole empty 4 KiB pages inside such units. Parked objects, metadata and the
  watermarks did not move. Every small class of 1 KiB or less switches to huge pages after
  16 MiB, and the purge returns only whole free 2 MiB units there. So a class whose demand
  falls keeps its peak. The sparse units are one contiguous tail at the top of each class's
  carved memory, above its dense units, and none are interleaved with them.
- **The same mechanism costs memory in three other places.** It holds the old classes' tails
  after a size shift (1.53 against jemalloc's 1.19 at a 384-byte median; 786 MiB of
  unreturnable free memory, all in sparse tails). It holds mini-redis's small-value classes
  when its keys are rewritten at changing sizes (1.33 against jemalloc's 1.16, still after a
  quiet minute). And it holds the low phases of the lifetime mix (507 MiB of empty pages in
  occupied units, 1.1 points of asmalloc's 4.2 against jemalloc's 3.8).
- **Large blocks lose for other reasons.** In a mix of 40 KiB to 32 MiB buffers, asmalloc
  ends three of four phases near 2.0 against jemalloc's 1.3 to 1.5. The excess is either
  resident pages inside occupied large slots beyond the blocks in them, 795 MiB at the end
  of the first phase, or free large slots kept resident while their regions keep receiving
  frees, 961 MiB at the end of the third. The second goes back within a quiet minute (1.11,
  jemalloc 1.09).
- **Memory is not bought with time, with one exception.** In the timed runs asmalloc took
  the fewest cycles per operation on steady, lifetime and large, and tied tcmalloc on shift.
  On swing, tcmalloc took 17% fewer (one round, on both L3 domains), where the long runs on
  one domain had them within 1.4%.
- **The ratchet has a rule now.** `docs/research/huge-tail.md` (main `49986cf`) takes huge-page
  advice off the stranded tail and returns its free pages. On this note's lifetime run it
  holds the peaks at 1.10 to 1.15 and cuts the troughs from 4.29 to 3.21. The shift and
  mini-redis show the same stranded tail and should respond to it; the large mix is
  something else.

## Method

**Workloads.** `workload drift shape=...` (`bench/workloads/src/drift.rs`), eight threads
pinned to the eight CPUs of `scripts/heavy` (12-15 and their siblings 28-31: one L3 domain).
Each run is a fixed number of operations split into epochs. At the end of each epoch every
thread stops at a barrier and one samples the process. Work and samples are counted in
operations, not time, so a paused process does the same work and is sampled at the same
points. Each thread has a seeded generator. Every value is written in full when it is made.

| Job | What it does | Live | Operations |
|---|---|---|---|
| steady | a cache shared by all threads: each operation puts a value (log-normal, median 128 B, sigma 1) into a random slot and evicts random entries while over budget, so most frees are another thread's object; two short-lived objects an operation (median 96 B) live for the thread's next 64 | 2 GiB | 15G |
| shift | the same, with the median moving 128, 384, 1024 B, one phase each, four cycles | 2 GiB | 12G |
| lifetime | each thread's FIFO of short-lived values alternates between 1.5 GiB and 128 MiB in total; one value in a thousand, of the same sizes, goes instead into a 384 MiB random-replacement cache (long-lived, turning over about once per phase pair) | 1.9 GiB at peaks, 0.5 GiB in troughs | 24G, 24 phases |
| swing | the steady cache filled by producers and drained by consumers through a queue; busy threads 8, 2, 8, 1, 8, 4 by phase, half of them producers | 1 GiB | 4.8G, 24 phases |
| large | a cache of buffers log-uniform in 40-256 KiB, 256 KiB-4 MiB, 4-32 MiB, then 40 KiB-32 MiB; half built by doubling from 4 KiB with `realloc` (half of those shrunk to fit), one operation in five grows a cached buffer by half | 1 GiB | 2.4M, 16 phases |
| mini-redis | `bench/apps/miniredis` preloaded with 1.2M keys (64 B to 4 KiB values); then nine phases, each overwriting every key once, in random order, at 80k commands a second (half SET, half GET): values 64 B-4 KiB, 32-256 B, 256 B-2 KiB, 32-256 B, and again | 0.3 to 1.3 GiB | 21.6M commands |

In the runs of the matrix each phase drew its own 4096-entry table of sizes (`tables=phase`).
A class that takes one draw in a hundred then has a share that moves by tens of percent from
phase to phase. That is what drives the ratchet on the lifetime mix, so the lifetime mix was
run again with one table for all phases (`tables=shared`, now the default) as the control.

**Measures.** `heap` is the process's `RssAnon` less the resident pages of the workload's
own bookkeeping, which it maps outside the allocator. `live` is the bytes the program holds,
at the sizes it asked for. The ratio is `heap` over `live`. After the load, the live set
stays and the process is sampled for a quiet minute. Later runs also record the run's cgroup
`memory.current`, which counts a partly unmapped huge page whole. In every run that has it,
it is within 1 to 2% of the resident set for every allocator but mimalloc (below).
For mini-redis, the server's `RssAnon` and cgroup are over the live bytes of a `live-bytes`
build's run at the same phase and second. The program's allocations do not depend on the
allocator, so one run of that build is every build's denominator.

**Breakdowns.** bench-alloc's memstat build of asmalloc walks the heap (`Asmalloc::inspect`)
and asks the kernel which pages are resident. A drift run with `account_every` asks it for a
full breakdown at chosen epochs while every thread is stopped, so the sums are exact. Free
memory is split by what a purge could return now: whole free 4 KiB pages outside a huge-page
part, and whole free 2 MiB units inside one. Each class's resident pages and units are counted
by occupancy, and each region's units are mapped in address order. The memstat build runs the
lifetime mix about nine times slower than the plain one, and reproduced its ratios to within
0.01 at every phase.

**Runs.** One run per allocator and job. Memory runs went one at a time through
`scripts/heavy` under `scripts/memlimit -m 8G`. Timed runs went through
`scripts/measure --both-l3` on CPUs 0-3 and 8-11, one job per session of about ten minutes
with a gap as long, the allocators in a new random order each round. Other teammates'
measurements froze the heavy slice for 2 to 13 minutes at a time. The drift runs, counted in
operations, only see such a freeze as a silence. A frozen mini-redis run loses part of a
phase, because the load generator's clock runs on, so it was detected (a gap in the sampler)
and run again.

## Results

Resident heap over live bytes. For jobs with phases, the value at the end of the last cycle's
phase, and in brackets the first cycle's where it differs by more than 0.03. "Quiet" is the
ratio after the quiet minute. The best of each row is in bold.

| Job | asmalloc | jemalloc | tcmalloc | mimalloc | glibc |
|---|---|---|---|---|---|
| steady | **1.105** | 1.148 | 1.127 | 2.969 | 1.266 |
| shift, median 128 | **1.134** (1.104) | 1.151 | 2.051 (1.126) | 2.442 (2.001) | 1.858 (1.183) |
| shift, median 384 | 1.533 | **1.188** | 2.051 (1.579) | 2.592 | 1.825 (1.489) |
| shift, median 1024 | **1.229** | 1.294 | 2.090 (2.004) | 2.110 (2.205) | 1.846 (1.745) |
| lifetime, peaks | 1.212 (1.091) | 1.140 (1.115) | 1.225 (1.121) | 1.289 (1.122) | **1.075** |
| lifetime, troughs | 4.307 (3.786) | **3.825** (3.614) | 4.592 (4.324) | 4.832 (4.321) | 4.028 (4.102) |
| lifetime, one size table, peaks | 1.107 | 1.123 | | | **1.074** |
| lifetime, one size table, troughs | 3.770 | **3.662** | | | 4.028 |
| swing | **1.223** (1.136) | 1.243, swings to 2.5 | 1.379 (1.147) | 1.985 | 2.345 (1.361) |
| large, 40-256 KiB | 2.078 (1.896) | **1.422** | 2.406 (1.177) | 2.209 | 1.500 |
| large, 256 KiB-4 MiB | **1.446** | 1.886 | 1.941 | 1.776 | 1.827 |
| large, 4-32 MiB | 2.067 | **1.290** | 2.207 | 2.169 | 2.588 |
| large, quiet | 1.109 | **1.085** | 2.012 | 2.351 | 2.391 |
| mini-redis, wide values | 1.33 (1.12) | **1.17** (1.10) | 1.23 (1.12) | 1.21 (1.16) | 1.25 (1.10) |
| mini-redis, small values | 2.55 (2.30) | 2.88 (2.50) | 5.06 | 4.90 | 4.81 |
| mini-redis, quiet | 1.33 | **1.16** | 1.21 | 1.19 | 1.24 |

Still rising when their runs end: tcmalloc on swing (about 0.01 a cycle) and glibc on the
shift (its median-128 phases 1.18, 1.72, 1.79, 1.86). mimalloc and tcmalloc ratchet on the
lifetime mix as asmalloc does (from 1.12 to 1.29 and to 1.23), and level off by the end.
Everything else is flat over its last half. The lifetime mix run three
times as long (72G operations) answers whether asmalloc's 1.21 is still climbing. Its peaks go
1.10, 1.11, 1.14, 1.16, 1.18, 1.17, 1.18, 1.20, 1.22, 1.21, 1.25, 1.21 over the first twelve
and then hold between 1.19 and 1.23 for 24 more, ending at 1.217. Its troughs go from 3.82 to
4.29 and then hold between 4.13 and 4.23. Over the same phases jemalloc holds 1.14 to 1.16
(3.76 to 3.84) for 27 peaks and glibc 1.074 to 1.076 (4.03) for 16. Those two runs stopped at
the 90-minute limit, at 52G and 30G operations.

mimalloc's cgroup charge on mini-redis is 10 to 15% above its resident set from the second
cycle on (for example 1.32 against 1.19 after the quiet minute). That is the cost the design's
huge-page rule avoids: partly returned huge pages the kernel has not yet split. Every other
allocator's charge is within 0.02 of its resident set.

## Where asmalloc's memory is

### The lifetime mix: a ratchet in huge-page units

The breakdowns below are from the memstat run of the lifetime mix with per-phase tables, at
the end of its second peak (once the long-lived cache is full) and of its last (MiB):

| | second peak | last peak | change |
|---|---|---|---|
| heap | 2123 | 2335 | +212 |
| asked for | 1922 | 1922 | 0 |
| live slots (with class rounding) | 2070 | 2066 | -4 |
| parked in magazines, spares and depot | 5 | 4 | -2 |
| free, returnable now | 4 | 9 | +5 |
| free, not returnable | 20 | 238 | +218 |
| above watermarks | 18 | 12 | -6 |
| metadata, magazines, slabs | 4.5 | 4.7 | +0.2 |

Of the 238 MiB that cannot be returned at the last peak, 176 MiB is whole free 4 KiB pages
inside 2 MiB units that hold something, and 62 MiB is free slots on pages that hold
something. At the last trough the same split is 587 MiB and 1004 MiB. The long-lived values
pin almost every page at the trough under every allocator, which is why all of them sit near
4. The 587 MiB of empty pages in occupied units is what asmalloc holds beyond a page-granular
allocator: about 1.1 points of its 4.3 against jemalloc's 3.8.

Per class, the memory carved and resident tracks the most the program ever asked of the
class, and never comes down (MiB, at the end of each peak in turn):

| Class | asked | carved |
|---|---|---|
| 896 | 69 78 89 92 87 74 112 82 81 73 78 69 | 76 85 96 100 100 100 122 122 122 122 122 122 |
| 1024 | 55 61 61 60 64 70 79 71 49 85 65 52 | 60 66 66 66 69 77 86 86 86 91 93 93 |
| 768 | 78 109 97 130 93 104 101 83 127 102 101 107 | 85 120 120 145 145 145 145 145 145 145 145 145 |
| 320 | 147 162 171 179 169 165 162 162 174 168 152 192 | 164 179 190 199 199 199 199 199 199 199 199 212 |
| 128 | 44 58 69 61 52 59 56 57 61 63 48 59 | 45 60 74 74 74 74 74 74 74 74 74 74 |

The classes above 1 KiB (1536, 2048), which stay on 4 KiB pages, do not ratchet: their free
pages go back.

**Where the sparse units are.** Each class's units of the huge-page part at the end of the
memstat run's last peak, from an independent run (12G operations; MiB from the region's base;
a unit is dense when more than half its carved bytes are not free, sparse at a quarter or
less). The highest dense unit stands in for the purge's frontier, which inspect does not
report. A teammate who exported the frontier found at least 99% of the stranded pages in units
at or above it at all 13 of their breakdowns, which these maps agree with:

| Class | carved | huge pages from | highest dense unit ends at | sparse units above it | sparse units below it |
|---|---|---|---|---|---|
| 768 | 146 | 16 | 116 | 16 | 0 |
| 896 | 99 | 16 | 80 | 9 | 0 |
| 128 | 74 | 16 | 62 | 6 | 0 |
| 448 | 136 | 16 | 130 | 3 | 0 |
| 96 | 60 | 16 | 56 | 2 | 0 |
| 112 | 70 | 16 | 68 | 2 | 0 |

The map of class 768, one character a unit (`F` full, `4` over three quarters, `1` a quarter
or less), is `FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF4444444444444444441111111111111111`. At the trough
the same regions are a few dense units at the bottom and sparse ones above (768:
`444421111111111...`, 61 sparse units).

**Why the holes are not refilled.** Fresh memory is not preferred. A fill carves only after a
search of the class's bitmaps from the top finds no free slot (`back/class.rs`,
`fill_within`), and in the table above a class carves only when its demand passes its old
peak. The holes are in classes whose demand has fallen, and the next peak's values in other
classes cannot use them, because a region holds one class. Memory moves between classes only
through the OS. In the huge-page part, which every class of 1 KiB or less enters after 16 MiB
(`policy::HUGE_AFTER_HUGE_PAGES`, `HUGE_MIN_SLOTS_PER_PAGE`), the purge returns only whole
free 2 MiB units. A unit with a few values still in it keeps all of its pages.

**Why glibc and jemalloc do not ratchet.** glibc does not separate sizes. A freed chunk merges
with its free neighbours and can serve a request of any size, so when one size is asked for
less and another more, the second reuses the first's memory in place, and nothing has to go
back to the OS. jemalloc separates sizes as asmalloc does, but its slabs are a few pages, and
empty slabs and dirty pages decay back to the OS in 4 KiB pages. The empty pages of a class
whose demand fell go back, and the class whose demand rose faults them in again.

**What it takes.** Free pages in a mostly empty 2 MiB unit have to go back as 4 KiB pages,
which gives up huge pages for that unit, against the design's rule "Never split a huge page".
Because the sparse units are always the tail of a class's carved memory, one boundary per
region is enough. `docs/research/huge-tail.md` builds and measures that rule, with the
kernel's side of splitting a unit, and is where this goes on from here.

### The size shift: the old classes' tails

At the end of a median-384 phase (one cycle of the shift, memstat build): heap 3087 MiB against
2049 asked (1.51; jemalloc 1.19). Of the heap, 786 MiB is free memory the purge cannot return,
424 MiB of it whole empty pages in occupied units. Against the median-128 phase before it, the
growth is new live data in the larger classes (2560, 3072, 1280 B and up: +880 MiB carved),
while the small classes that phase had filled keep their memory. For example, class 160 B
asks for 25 MiB and holds 128, class 96 B asks for 7 and holds 63. Their units are a few dense
ones at the bottom and a tail of sparse ones (160 B: `FFFF3` then 51 units a quarter full or
less; 96 to 128 B: every unit a quarter full or less). It is the lifetime mix's mechanism with
a larger swing in demand: the same stranded tail, which the tail rule addresses. The next
median-128 phase refills those tails, so the ratio comes back to 1.13 and does not drift
across cycles.

### mini-redis: the same, in a real server

After the first cycle, asmalloc's ratio at the end of each wide-value phase is 1.31 to 1.34,
against 1.12 in the first. It stays at 1.33 after a quiet minute, while jemalloc returns to
1.16. At the end of that minute the memstat build holds 1371 MiB of live slots against 1299
asked, 4 MiB parked, and 349 MiB of resident free slots that the minute did not return. The
small-value phases fill the classes of 32 to 256 B, and the wide phases leave their tails
sparse: the same stranded tail as the lifetime mix, not yet measured with the tail rule. In
the small-value phases themselves asmalloc is with jemalloc (2.3 to 3.1) and well under the
others (about 5).

### Large blocks: what earlier tenants left, and the hold

The large mix's excess is not in huge-page units, and not the ratchet, so the tail rule does
not touch it. Large classes over
the load (memstat build, MiB):

| End of phase | heap | live large slots | asked in large blocks | resident beyond the blocks | free large slots resident |
|---|---|---|---|---|---|
| 1: 40-256 KiB | 1940 | 1819 | 1025 | 795 | 116 |
| 2: 256 KiB-4 MiB | 1493 | 1125 | 1025 | 100 | 360 |
| 3: 4-32 MiB | 2025 | 1056 | 1010 | 46 | 961 |
| 4: 40 KiB-32 MiB | 1963 | 1129 | 1024 | 105 | 825 |
| after a quiet minute | 1163 | 1129 | 1024 | 105 | 29 |

In the first phase no memory is on huge pages, and the excess grows through the phase while
live stays at 1 GiB. It sits in occupied slots, mostly of the 1.25 MiB class, which no request
of that phase asks for: buffers grown by doubling move into slots with 16 times their room.
The likely mechanism is the design's "a smaller tenant after a larger one is charged what the
larger one left". A buffer appended to fills its roomy slot and moves on. Under this churn
the next growing buffer claims the freed slot before the hold returns it, so the pages past
the new block stay resident as long as the slot is occupied. That is inferred, not traced.
In the third phase the excess is free slots of the 5 to 28 MiB classes kept resident: each
region keeps receiving frees, so the hold that returns its free slots does not mature while
the load runs. Those go back within the quiet minute.

## Throughput

Cycles per operation over the load (all threads, user and kernel), from shortened runs of
each job on CPUs 0-3 and 8-11 (both L3 domains, one thread a core), under
`scripts/measure --both-l3`. Two rounds of steady and lifetime, one of the rest; the rounds
of a job agree to within 0.5%. The jobs are shorter than the memory runs: steady 1.5G
operations, shift one cycle of 0.6G, lifetime 2.4G in four phases with five long-lived values
in a thousand, swing 0.48G in six phases, large 240k operations in four phases.

| Job | asmalloc | jemalloc | tcmalloc | mimalloc | glibc |
|---|---|---|---|---|---|
| steady | **1271** | 1398 (1.10) | 1326 (1.04) | 2064 (1.63) | 2051 (1.61) |
| shift | 2335 | 2483 (1.06) | **2315** (0.99) | 3016 (1.29) | 3455 (1.48) |
| lifetime | **595** | 634 (1.07) | 619 (1.04) | 764 (1.28) | 888 (1.50) |
| swing | 3873 | 3918 (1.01) | **3218** (0.83) | 4134 (1.07) | 7863 (2.03) |
| large (millions) | **8.3** | 13.6 (1.64) | 14.5 (1.74) | 12.8 (1.53) | 10.0 (1.20) |

The long memory runs, on one domain with SMT siblings busy, rank the same way, with one
difference: on swing asmalloc and tcmalloc were within 1.4% there (2744 and 2706). Which part of
the swing's producer and consumer traffic tcmalloc serves more cheaply across two domains is
not investigated here.

## Caveats

- The runs compress hours into minutes. Rules that act on the clock (asmalloc's intervals,
  its hold, jemalloc's decay) act less often per operation than in a server running at a
  tenth of this rate, so returning memory is if anything under-represented here.
- The memory runs use one L3 domain (the cores `scripts/heavy` allows), so asmalloc's depot
  has one domain stack where a server on both L3s would have two. The timed runs use both.
- One run per allocator and job. Repeats agree closely where they were made: asmalloc's three
  lifetime runs with per-phase tables (the matrix's, the long one's first 24G operations and
  the memstat build's) match to within 0.012 at every peak.
- The lifetime mix's per-class wander comes from sampling noise in the size tables, about
  20 to 30% for a class that takes one draw in a hundred. A real program's mix may wander more
  or less. The bound of the ratchet is set by how far each class's demand ever rises above
  its usual level.

## Reproduce

From the repository root, on a checkout of this branch:

```
scripts/heavy bench/probes/heap-drift/build.sh            # workload once per allocator, and the memstat build
scripts/heavy bench/probes/heap-drift/matrix.sh OUT "steady shift lifetime swing large" "asmalloc je tc mi glibc"
scripts/heavy bench/probes/heap-drift/matrix.sh OUT/shared lifetime "asmalloc glibc je" tables=shared
scripts/heavy bench/probes/heap-drift/matrix.sh OUT/long lifetime "asmalloc glibc je" ops=72G phases=72 samples=432
scripts/heavy bench/probes/heap-drift/build-miniredis.sh   # mini-redis once per allocator, live-bytes, memstat; loadgen
python3 -I bench/probes/heap-drift/requests.py target/heap-drift/redis-inputs 1200000
scripts/heavy bench/probes/heap-drift/accounts.sh OUT/acct  # breakdowns, then the mini-redis runs
scripts/measure --both-l3 bench/probes/heap-drift/timed.sh OUT/timed steady 2   # and lifetime, swing, shift, large
python3 -I bench/probes/heap-drift/summary.py OUT --phases
python3 -I bench/probes/heap-drift/summary.py OUT/timed --timed
python3 -I bench/probes/heap-drift/redis_summary.py OUT/acct/redis
python3 -I bench/probes/heap-drift/account.py OUT/acct/lifetime/lifetime.asmalloc-memstat.txt.memstat --diff 3,11
```

`account.py` also takes `--history TAGS --sizes S,...` (per-class demand against carved) and
`--where TAG` (the unit maps). The raw data of this note is in
`docs/research/data/heap-drift-2026-10-24.tar.gz`: the matrix (`main/`), the fixed-table
control (`shared/`), the long runs (`long/`), the breakdown runs and their memstat files
(`acct/`, `acct2/`, with the mini-redis runs and those a freeze disturbed in
`acct2/redis/frozen/`), and the timed runs (`timed/`).
