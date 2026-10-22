# Meilisearch's tail with purge work on the background thread

2026-10-22, on the reference host (Ryzen 9950X, Linux 7.2.9). The question is whether moving
purge work from request slow paths to the background thread (main `1eea9af`, merged from
`purge-off-path`; its parent without the change is `2754021`) costs Meilisearch tail latency
at 70% load. `docs/research/purge-off-path.md` left this unresolved after six runs a build.

## Verdict

**No tail cost is shown.** The comparison that carries the verdict uses one binary that
switches the behaviour at run time. Within it, moving purge to the thread changes p99 by
-0.9 ms (95% interval -2.1 to +0.5) and p99.9 by 0.0 ms (-2.3 to +2.3), over 14 interleaved
rounds of two minutes at 70%. So any cost is no larger than about 0.5 ms (2%) at p99 and
2.3 ms (5%) at p99.9. p50 is 0.02 ms lower with the thread (-0.046 to -0.001), and cycles a
request are level (-0.7%, interval -1.9% to +0.6%).

**The separate builds differ, and the switch does not reproduce that.** Main's build against
its parent's build reads +2.0 ms at p99.9 (+0.6 to +3.4) and +0.7 ms at p99 (-0.2 to +1.6).
That is a difference between two binaries, not the change: the same behaviour switched in one
binary shows nothing, and two binaries with the same behaviour (the switch build in
slow-path mode against the parent's build) differ by +0.8 ms at p99.9 (-1.3 to +3.4) on
their own. The likely causes are where code lands (`grpc-placement.md`) or chance across three
comparisons. Nobody has tested which.

**Page faults** in the switch build are 5% higher with the thread: 87.3 against 82.7 a
request, higher in 11 rounds of 14, with a paired difference of +4.6 (+1.7 to +7.0). This
matches the first round of `purge-off-path.md` and costs no latency here. Main's and the
parent's builds are level (82.5 each), so whether the rise is real is not settled.

## Tables

`meili-tail` suite, 70% (930 requests a second), `s70`: 120 s, about 111,600 requests a run.
The "run" columns give the median of runs' quantiles, with the range. "Pooled" is the quantile
of every request of all 14 runs. Cycles and faults are means a request over the segment. The
thread columns come from the watcher.

| build | p50 ms | p99 ms | p99.9 ms | pooled p50 / p99 / p99.9 | kcycles a request | faults a request | thread wake-ups / CPU ms a second | TLB shootdowns a second a CPU |
|---|---|---|---|---|---:|---:|---|---:|
| switch, thread mode (main's behaviour) | 1.80 (1.72 to 1.83) | 29.9 (27.6 to 32.5) | 46.4 (42.6 to 52.2) | 1.79 / 30.2 / 47.0 | 17,004 | 87.3 | 25.0 / 8.8 | 370 |
| switch, slow-path mode (the parent's behaviour) | 1.82 (1.73 to 1.86) | 31.1 (28.1 to 34.1) | 46.2 (43.5 to 56.1) | 1.82 / 31.0 / 47.2 | 17,121 | 82.7 | 2.0 / 0.0 | 366 |
| main `1eea9af` | 1.82 (1.73 to 1.89) | 31.4 (26.9 to 35.0) | 48.0 (43.8 to 53.7) | 1.81 / 31.3 / 48.3 | 17,014 | 82.5 | 26.2 / 7.7 | 368 |
| parent `2754021` | 1.82 (1.75 to 1.88) | 30.2 (26.9 to 34.1) | 45.8 (42.4 to 49.6) | 1.82 / 30.5 / 46.4 | 17,123 | 82.5 | 2.0 / 0.0 | 368 |
| jemalloc | 2.07 (1.99 to 2.12) | 36.8 (31.8 to 38.9) | 53.9 (47.8 to 57.9) | 2.07 / 36.7 / 54.0 | 17,669 | 38.8 | | 602 |

Paired by round, as the mean difference of runs' quantiles, with a 95% interval from
resampling rounds:

| comparison | p50 ms | p99 ms | p99.9 ms | kcycles | pooled p99.9 ratio |
|---|---|---|---|---|---|
| switch: thread against slow-path mode | -0.024 (-0.046 to -0.001) | -0.87 (-2.13 to +0.52) | +0.01 (-2.29 to +2.27) | -117 (-330 to +105) | 0.996 (0.943 to 1.050) |
| main against parent (two builds) | -0.008 (-0.023 to +0.006) | +0.73 (-0.20 to +1.64) | +1.96 (+0.56 to +3.41) | -109 (-271 to +31) | 1.042 (1.014 to 1.071) |
| switch slow-path mode against parent (same behaviour, two builds) | +0.000 (-0.019 to +0.019) | +0.54 (-0.81 to +1.99) | +0.83 (-1.26 to +3.36) | -2 (-187 to +157) | 1.018 (0.966 to 1.078) |
| jemalloc against parent | +0.251 (+0.233 to +0.266) | +6.07 (+4.71 to +7.50) | +7.40 (+5.28 to +9.53) | +546 (+438 to +673) | 1.165 (1.120 to 1.210) |

**TLB shootdowns.** The thread's `madvise` calls do not add shootdowns. Each server CPU takes
about 367 a second in every asmalloc arm (jemalloc 600). The thread makes them a little more
bursty. The p99 of one CPU's count in 100 ms is 93 to 96 with the thread and 87 without, and
that is not visible in latency. The two untested mechanisms the lead named, a woken thread
running ahead of its waker and `madvise` bursts, were not chased, because there is no
difference for them to explain.

## Method

**The switch.** `bench/probes/meili-tail/switch_edit.py` patches a scratch copy of main.
When the background thread is created, it reads `ASMALLOC_DIAG_SLOW_PURGE`. If the value is
`1`, it does two things. `Snapshot::duty` always answers `Duty::Slow`, so slow paths take
every purge step and wake nobody. The thread runs the parent's loop: `maintain` rounds and a
timed sleep, with no purge steps and no `SLEEPING` mark. Any other value leaves main as it
is. `build-at.sh asmalloc-switch 1eea9af switch_edit.py` builds that one binary, and
`build-at.sh asmalloc-parent 2754021` builds the parent's. The two modes are two local
`bench/allocators.toml` entries on the same build, with environments of equal length
(`=0` and `=1`). The watcher confirms each mode: 2 wake-ups a second and no thread CPU in
slow-path mode, as on the parent, and 25 and 9 ms in thread mode, as on main.

**What it costs.** The `meili-tail` job is the large-heap fill, then 120 s at 930 requests a
second, with no silence or burst. A run takes 155 s, mostly Meilisearch indexing its 400k
items at start. `session.sh` runs one round, the arms in random order, under
`scripts/measure --both-l3`, with the load generator's record of every request
(`BENCH_RAW_DIR`), the watcher and `irqs.py` beside it. A round of five arms is 13 minutes.
`tail.py` gives the tables. On this job the per-run p99.9 spreads over about 10 ms (p99 over
about 6 ms) for any one build. Fourteen paired rounds, 3 h of measurement with five arms,
gave intervals of about ±1.4 ms at p99 and ±2.3 ms at p99.9. Halving them would take four
times as many rounds. Separate builds of the same behaviour already differ by about 1 ms at
p99.9, so a tail comparison on Meilisearch needs the change switched in one binary, not two
builds.

Raw results: `docs/research/data/meili-tail-2026-10-22.tar.gz` (benchrun's records and the
two tables). The request records, watcher and interrupt samples (320 MB) are under
`target/meili-data/` of the worktree that made them. Round 15 was stopped at its start and
is not used.
