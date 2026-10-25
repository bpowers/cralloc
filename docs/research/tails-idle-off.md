# Server tails with deep idle off

2026-10-25, reference host (Ryzen 9950X, Linux 7.2.9), asmalloc at main `3b5a3f5` against
jemalloc, tcmalloc (per-CPU), mimalloc and glibc. The standings measured server tails at one
fixed rate with the CPUs' deep idle states on, and `h2-tail.md` showed that this can rank idle
states instead of allocators. Here C2 and C3 are off on the server CPUs, so a tail is the
server's own. Only the two HTTP/2 Connect jobs are done so far; the rest wait for the
measurement lock (below).

## Summary

- **The Connect over HTTP/2 tail was the idle states.** With deep idle off, asmalloc's p99.9
  on `connect log h2` is 37.8 us, against 45.1 in the standings. The incumbents did not
  move (within 0.4 us). asmalloc now has the lowest p99.9: mimalloc 38.6, tcmalloc 39.2,
  jemalloc 39.7, glibc 41.6. It is lower at every one of the five placements against every
  incumbent, resolved with ranges apart.
- **gRPC over HTTP/2 did not change**, as expected: its standings runs had mostly been in the
  fast mode. asmalloc's p99.9 is 38.2 us (38.3 before), against tcmalloc 39.2, mimalloc 39.5,
  jemalloc 40.2 and glibc 41.6, all resolved.
- **p50 and p99 are the best on both jobs**, as before, by 2.5 to 10%, at every pad.
- **The gaps are small**, 2 to 10% at p99.9. Every one is resolved, but none comes near the
  14% deficit the standings showed.
- **No real tail deficit for asmalloc on these jobs.** One asmalloc run of twelve reached
  43.8 us (below).
- **Cycles a request and saturated rates moved by under 1%** for every allocator. So numbers
  from before and after the switch can be compared on these jobs. Only the tails of runs that
  had gone deep differ.

## Method

- **Idle setting.** Bobby disabled C2 (18 us exit) and C3 (350 us) on CPUs 0 to 3, the
  servers' CPUs, as root. `scripts/deep-idle status` showed them disabled at the start of the
  work. `bench/probes/tails-idle-off/guard.sh` ran it again before every session, logged the
  output, and would have stopped the queue had any deep state been enabled. benchrun records
  the setting per run as well: all 98 runs have both states disabled on all four CPUs for the
  whole of s70, and 0 deep entries. The CPUs still enter POLL and C1 (1 us), about 1,000
  to 1,600 C1 entries a thousand requests. The SMT siblings 16 to 19 keep their deep states, but they
  run benchrun's spinners as in the standings. The load generator's CPUs busy-poll.
- **Builds.** Every allocator was built from this checkout at bench-alloc's five layout pads
  (0, 16, 32, 48, 64; `bench/probes/standings/pad-build.sh workloads`), as the standings were.
  All five asmalloc builds pass `scripts/rseq-offset-check`.
- **Runs.** The servers suite's day, ranked at s70 (63k req/s, the standings' rate). One round
  a session, the pads interleaved within it, through `standings/queue.sh` and `session.sh`
  under `scripts/measure --both-l3`. Connect h2: rounds 1 to 3 at pads 00 to 48 and rounds 1
  and 2 at pad 64, so 12 runs for each of the four, and one round of glibc (5 runs). gRPC h2:
  two rounds (10 runs), glibc one (5). The third gRPC round was stopped before it started,
  to free the lock. No run was invalid.
- **Analysis.** `bench/probes/tails-idle-off/tails.py`. A percentile is the median over runs,
  with the lowest and highest run. A difference is resolved when the two allocators' ranges
  of per-pad medians do not overlap, or an exact permutation test of those medians gives
  p < 0.05 (`standings/servers.py`'s rule for cycles). The old figures are the standings'
  runs (`standings-2026-10-23.tar.gz`) through the same script.
- Raw data: `docs/research/data/tails-idle-off-2026-10-25.tar.gz` (benchrun's records, the
  session logs with each session's idle setting, the analysis as JSON).

## Results

p50 / p99 / p99.9 in us at s70: medians over runs, with p99.9's lowest and highest run. Old
is the standings' figure with deep idle on.

| Job | Allocator | Runs | Deep idle off | p99.9 range | Old (deep idle on) | p99.9 by pad, medians (00, 16, 32, 48, 64) |
|---|---|---|---|---|---|---|
| connect log h2 | asmalloc | 12 | 28.9 / 35.8 / **37.8** | 36.9 to 43.8 | 29.4 / 36.5 / **45.1** | 37.1, 38.3, 37.7, 38.1, 37.8 |
| | mimalloc | 12 | 29.8 / 36.8 / 38.6 | 38.4 to 39.6 | 30.0 / 37.1 / 39.0 | 38.6, 38.6, 39.2, 39.0, 38.5 |
| | tcmalloc | 12 | 30.3 / 37.2 / 39.2 | 38.0 to 39.9 | 30.4 / 37.3 / 39.4 | 39.5, 38.4, 39.3, 38.5, 39.6 |
| | jemalloc | 12 | 31.1 / 37.9 / 39.7 | 39.4 to 40.3 | 31.0 / 37.9 / 39.8 | 39.8, 39.5, 40.2, 39.5, 39.8 |
| | glibc | 5 | 32.4 / 39.2 / 41.6 | 40.5 to 41.8 | 32.5 / 39.2 / 41.4 | 40.9, 41.7, 41.6, 41.8, 40.5 |
| connect log grpc h2 | asmalloc | 10 | 29.5 / 36.2 / **38.2** | 37.6 to 38.4 | 29.6 / 36.5 / 38.3 | 38.3, 38.0, 37.7, 38.1, 38.3 |
| | tcmalloc | 10 | 30.4 / 37.4 / 39.2 | 37.0 to 40.9 | 30.5 / 37.3 / 39.1 | 38.2, 38.5, 39.2, 39.0, 40.0 |
| | mimalloc | 10 | 30.5 / 37.5 / 39.5 | 38.2 to 1,235 | 30.1 / 37.2 / 39.8 | 637.5, 38.5, 39.5, 40.5, 38.8 |
| | jemalloc | 10 | 31.2 / 38.1 / 40.2 | 39.1 to 43.6 | 31.2 / 38.0 / 39.8 | 43.0, 39.6, 40.7, 39.4, 40.2 |
| | glibc | 5 | 33.1 / 39.6 / 41.6 | 38.8 to 42.2 | 33.0 / 39.3 / 41.3 | 38.8, 41.8, 41.6, 42.2, 40.8 |

Against asmalloc, p99.9: on Connect h2, mimalloc +2.0%, tcmalloc +3.6%, jemalloc +4.9% and glibc
+9.9%, every one resolved with ranges apart and asmalloc lower at 5 of 5 pads. On gRPC h2,
tcmalloc +2.8% (permutation p = 0.024, asmalloc lower at 4 of 5 pads), mimalloc +3.5%,
jemalloc +5.4% and glibc +9.0% (ranges apart, 5 of 5). At p99, every incumbent is 2.6 to 9.3%
above asmalloc, at every pad of both jobs.

**What changed.** asmalloc's Connect h2 p99.9 fell by 7.3 us, and nothing else moved by more
than 0.5 us at any percentile. That is what `h2-tail.md` predicted: the extra 0.1% of slow
requests was C2 and C3 exits, which the faster server ran into because it idles longer. The
standings ranked asmalloc last on this job's p99.9, and it is now first. On gRPC its
standings runs had stayed out of the idle mode, so the figure is the same.

**The two odd runs, as they are.**
- asmalloc pad 16, round 1, Connect h2: p99.9 43.8 us, with a p99 of 35.8 like the other
  runs, a maximum of 217 us, no missed slots, no deep entries, and no work on its CPUs from
  anything else (`foreign_by_segment` under 0.03 at s70). The other eleven asmalloc runs are
  36.9 to 38.6, and the same pad's other runs 38.3 and 37.8. Not explained. One run in
  twelve does not move the median or the verdicts.
- mimalloc pad 00, round 1, gRPC h2: p99.9 1,235 us. The run had a stall of about 3.4 ms
  (maximum 3,447 us, 306 missed slots), and the runner recorded no other work on its CPUs.
  Two jemalloc runs at pad 00 had similar 3.3 to 3.6 ms maxima, with fewer requests caught
  in them (p99.9 43.6 and 42.4). What stalls is not known; it is not specific to mimalloc. It is kept, and the medians are robust
  to it. It inflates mimalloc's mean over pads (the pad-means figure in the script's output),
  and that mean is not quoted here.

**Cycles and saturation, before and after.** Cycles a request at s70, mean over pads, new
against old: Connect h2 asmalloc 124.0k against 125.0k (-0.8%), the incumbents -0.4 to
+0.2%; gRPC h2 -0.7 to +0.2% for every allocator. The day's closed-loop segment (4 s, every
run) gives the saturated rate: within 0.8% of the standings' runs for every allocator (Connect h2
asmalloc 198.8k against 197.7k req/s). The servers' CPUs are busy at saturation, so this is
expected. At 63k they idle between requests, but in C1 or POLL as often as in C2 before, so
the work a request is the same. The cycle standings stand as measured. `h2-tail.md`'s spinner
runs, where the CPUs never idled, cost 2 to 3% fewer cycles. That effect comes from not
idling at all, not from which idle state is used, and this setting does not produce it.

## Not yet re-measured with deep idle off

The queue was stopped here to free the measurement lock for the team, and the setting lasts
until reboot. Still to run, as planned (two rounds over five pads, glibc one):

- the third round of gRPC h2;
- `connect log` (HTTP/1.1), `orders axum`, `orders actix`, `orders axum h2`, `sozu proxy`,
  `miniredis`, `router graphql` and `router graphql fast`;
- `connect store 1M` and `orders axum cache 2G` (servers-large);
- `rpc echo c64` and `rpc log c64` (kernel suite, closed loop: p50 and p99 only);
- Meilisearch (`meilisearch hn thread`, single builds, as the standings);
- `servers-sat` and `servers-large-sat`, for the saturated rates in their own suite.

SurrealDB and Qdrant are left out. Their builds on disk carry an older asmalloc (and a
prototype switch). Their tails are in milliseconds, which an 18 us exit cannot move. A
relink takes an hour per allocator.
