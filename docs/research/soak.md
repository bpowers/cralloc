# Soak: Qdrant for ten and a half hours

Run 2026-10-27 to 2026-10-28, asmalloc at main `8e9d097` (the tail rule, the large-slot rules,
the background thread and the published region tags), against jemalloc (Qdrant's own default)
on the same schedule at the same time on the same machine.

**Nothing drifted.** No crash, no abort, no error that jemalloc's run did not have. Resident
memory at the quiet points rose for the first fifteen cycles in both runs and then stayed
flat, asmalloc's about 20 MiB below jemalloc's throughout. Mappings stayed bounded, the
allocator's count stayed within a few MiB of what `mincore` sees, and huge pages did not drain.
The background thread used 9.8 s of CPU in 10.6 hours, and its wake-ups tracked Qdrant's own
activity during the silences.

The schedule ran long: the shared cores were often frozen by other measurements, which froze
the runs too, so the first 35 cycles, planned at 6.5 hours, took 10.6. The session was wound
down, so both runs were ended cleanly at the end of cycle 34's quiet stretch, after 35 of 61
cycles and 10.6 hours.
The Connect/h2 RPC soak was not run.

## Method

- **Server.** Qdrant 1.20.0 (`bench/apps/qdrant`) over its million-vector collection,
  copied to disk (not `/dev/shm`) at the start of each run. asmalloc's build has the memstat
  sampler (`bench/alloc/src/memstat.rs`), sampling every 20 s. The jemalloc build is the
  suite's `je` build.
- **Isolation.** Each server ran through `scripts/heavy` under `scripts/memlimit -m 5G`.
  The 2 GiB the brief suggested does not fit: the cgroup carries the collection's file pages,
  about 1.8 GiB, besides the heap. Peaks were 4.3 GiB per run, most of it page cache. The
  load generators ran through `scripts/heavy` too, on the same cores as other teammates'
  builds and paused by their `--both-l3` measurements.
- **Schedule.** `soak.py` drew 61 cycles from seed 1, the same for both runs. Each cycle is one
  to three load phases of 60 to 300 s, each a separate `loadgen` run, so connections open and
  close between phases. The phase kinds are:
  - searches and the suite's mixed load, at 50 to 200 req/s over 2 to 64 connections;
  - closed-loop searches on 2 to 8 connections;
  - upserts of new points at 5 to 20 req/s, and deletes of the same points in the same cycle;
  - every fourth cycle, a second collection created, filled with 8,000 points (enough for
    Qdrant to index it), searched, and dropped.

  Each cycle then has a quiet stretch with no connections, of 1 to 8 minutes, and one in six
  of 10 to 20 minutes. Writes do not wait for indexing (`wait=false`); writes that waited
  queued behind the optimizers for minutes on the shared cores.
- **Series**, every 10 s: `smaps_rollup`, the number of mappings, minor and major faults, the
  process's CPU time and the `asmalloc` thread's, and the run cgroup's `memory.current` and
  `memory.stat`. At the end of each quiet stretch, the last sample is recorded, along with the
  thread's CPU share over the stretch's second half and the sampler's last line. The sampler's
  line gives `free` (free slots on pages `mincore` calls resident) against `retained` (what the
  allocator counts).
- **Checks.** The asmalloc run checked itself against jemalloc's files as it went, and was
  set to stop at any finding the brief lists. None fired.

Scripts in `bench/probes/soak/`:
- `build.sh`: the builds.
- `qdrant_requests.py`: the request files.
- `soak.py`: the schedule, sampler and checks.
- `run.sh`: one run.
- `summary.py`: the tables below.

The connect schedule in `soak.py` (the store's mix over HTTP/1.1, and ingest over HTTP/2 with
batches of 5 to 500 records) passed a short trial and has not been run for long.

Data: `docs/research/data/soak-2026-10-28.tar.gz`. Each run's directory holds the series, the
phases, the quiet points and the schedule; asmalloc's also holds the sampler's output. The
tarball also holds the script that ended the runs.

## Results

At the quiet points, medians over each group of five cycles, MiB. "Count less mincore" is
asmalloc's `retained` less `free`:

| cycles | anon asm / je | RSS asm / je | cgroup asm / je | mappings asm / je | thread CPU, max | count less mincore |
|---|---|---|---|---|---|---|
| 0-4 | 136 / 149 | 1672 / 1756 | 1815 / 1918 | 734 / 474 | 0.07% | +0.5 to +5.0 |
| 5-9 | 142 / 173 | 1933 / 1932 | 1950 / 1988 | 752 / 523 | 0.07% | +2.3 to +3.5 |
| 10-14 | 146 / 176 | 1971 / 2004 | 1998 / 2046 | 772 / 538 | 0.08% | +2.6 to +3.8 |
| 15-19 | 155 / 177 | 1959 / 2004 | 1984 / 2052 | 758 / 557 | 0.07% | +1.9 to +4.6 |
| 20-24 | 156 / 176 | 1947 / 2009 | 1973 / 2047 | 773 / 559 | 0.07% | +2.7 to +4.7 |
| 25-29 | 156 / 176 | 1951 / 1981 | 1985 / 2025 | 758 / 543 | 0.07% | +1.9 to +3.6 |
| 30-34 | 155 / 178 | 1962 / 1987 | 1988 / 2025 | 778 / 561 | 0.08% | +3.1 to +4.7 |

Under load, the peaks of each group of five cycles, MiB:

| cycles | peak anon asm / je | peak AnonHugePages asm / je |
|---|---|---|
| 0-4 | 574 / 652 | 180 / 0 |
| 5-9 | 734 / 762 | 296 / 0 |
| 10-14 | 673 / 199 | 234 / 0 |
| 15-19 | 173 / 200 | 16 / 0 |
| 20-24 | 168 / 194 | 16 / 0 |
| 25-29 | 167 / 195 | 16 / 0 |
| 30-34 | 164 / 198 | 16 / 0 |

- **Quiet memory.** It rose in both runs over the first fifteen cycles (asmalloc 136 to 155
  MiB, jemalloc 149 to 177) and was flat for the 20 cycles after. The margin, asmalloc below
  jemalloc by 13 to 31 MiB, did not grow. RSS and the cgroup's current are mostly Qdrant's
  file pages and level between the runs.
- **Peaks.** The peaks of 570 to 760 MiB are the optimizers rebuilding segments after
  deletes and while indexing the second collection. They fall in delete and scratch phases in
  both runs, and only in the first 15 cycles in both; later rebuilds fell outside the sampled
  phases. They are the workload's, not the allocator's.
- **Huge pages.** asmalloc's AnonHugePages under steady searches was 16 MiB from the first
  cycle to the last, rising with those rebuilds, so it did not drain. khugepaged collapsed 83
  huge pages for the run's cgroup, and 1,277 huge pages were faulted in whole. jemalloc's run
  used none, under the system's `madvise` mode.
- **Mappings.** asmalloc's process had 515 to 962 mappings over the run and about 770 at the
  quiet points from cycle 10 on; jemalloc's had 314 to 719. Advice changes split mappings, which
  accounts for the ~200 more, and the number did not climb.
- **Count against `mincore`.** In all 920 samples, the count was between 0.6 MiB under and
  6.8 MiB over what was resident, median 3.3 MiB over; at the quiet points, 0.5 to 5.0 over. An
  over-count costs a later pass a call on pages that are gone, and it did not grow.
- **The background thread.** It used 9.8 s of CPU in 10.6 hours. Over the second half of a
  quiet stretch it took about 0.07% of a CPU when Qdrant's own threads were busy there (0.1 to
  0.4% of a CPU: its flushes and timers), and under 0.01% when Qdrant was idle, as in cycles
  2, 9, 17, 19 and 26. So it parked when the process was silent and woke with Qdrant's own
  slow paths, as the design says.
- **Errors.** Both runs counted about 120,900 failed requests in the same phases, within a few
  requests of each other. All of them are requests a phase still owed when its time ran out:
  phases frozen by other measurements ran 30 to 300 s past their plan, and the load generator
  counts the backlog it never sent as failures. jemalloc's run also had one reconnect in an
  upsert phase. Neither run had an error response.
- **Faults.** Over the run, asmalloc's process took 2.7 million minor faults to jemalloc's
  4.2 million, and 1.7 million major faults to 1.2 million. The major faults are Qdrant
  reading its segment files back after the optimizers rewrite them.
- **Ending.** Both drivers checked the collection and stopped Qdrant cleanly. Their final
  report then failed on a closed pipe, because the run scripts had been stopped first.

## Not resolved

- What the soak cannot show in 10.6 hours of a mostly read workload: a heap that grows for
  days, or a fragmentation pattern built by weeks of churn.
- The RPC server's soak, and a soak with the server's cores to itself: the latency series of
  this one mean nothing, and the freezes stretched the schedule by two thirds.
