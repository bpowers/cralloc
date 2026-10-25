# huge-tail probes

For `docs/research/huge-tail.md`. Nothing here is product code.

- `kernel.c`: what Linux does with part of a transparent huge page returned, how to force the
  split, what khugepaged does with advice off and on, what an advice change costs other
  threads, and how many mappings a region keeps. No allocator. Experiments: `partial`,
  `pressure`, `split`, `khuge`, `stall`, `maps` (see its header).
- `ht_edit.py`, `ht_proto.py`, `ht.rs`: the run-time switch (`ASMALLOC_HT`, documented in
  `ht.rs`) that advises the tail of a region's huge-page part against huge pages again and
  purges it as small pages; and the frontier and stranded pages on heap-drift's memstat unit
  maps. `ht_edit.py TREE` applies them to a scratch copy.
- `build.sh NAME COMMIT [PART...]`: extracts COMMIT, applies heap-drift's bench changes and the
  switch, and builds the drift workload (plain and memstat), the RPC servers, apps, or the
  incumbents' workload under `bench/target/NAME`. `KEEP=1` keeps the edited tree. Run under
  `scripts/heavy`.
- `drift.sh`: one drift run under a cap, with the cgroup's and the kernel's huge-page counters
  and the free 2 MiB blocks around it. `free2m.sh`: the free blocks of 2 MiB or more.
- `where.py`: from the memstat maps, the stranded empty pages above and below the frontier.
- `summ.py`: drift runs by arm: resident over live at the peaks and in the troughs (RSS and
  cgroup), huge pages at the peaks, cycles per operation, walks, counters.
- `thpwatch.sh`, `thpsumm.py`: the server sessions' huge pages, sampled from `smaps_rollup`.
- `htsumm.py`: the switch's counters (`d1`). `rampcost.py`: the climb out of each trough.
- `runner_edit.py BENCH_DIR`: adds the `ht-bursts` suite (bursts after lulls with a trickle of
  traffic) to a copy of the runner; applied to this checkout's `bench/` for the sessions and
  reverted, not committed.
- `sessions/`: the measurement sessions, run from the checkout under `scripts/measure`
  (`kern.sh`, `swing.sh`, `swingsize.sh`, `servers.sh`, `servers-watch.sh`) or `scripts/heavy`
  (`long.sh`). The server sessions need local `bench/allocators.toml` entries (not
  committed), one per arm, all `feature = "asmalloc"` with `build` the switch build and
  `env = { ASMALLOC_HT = "..." }` (and `HT_DIAG` for counters): `ht-off`, `ht-on`, `ht-p30`
  (`ht6`: `t0`, `t1d1`, `t1p30d1`), `h8-*` (`ht8`: `t0`, `t1d1`, `t1g1d1`, `t1p30d1`), `h9-*`
  (`ht9`: `t0`, `t1d1`, `t1r1d1`), `h10-*` (`ht10`: `t0`, `t1r2d1`, `t1r2`).
