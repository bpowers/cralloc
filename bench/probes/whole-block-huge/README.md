# whole-block-huge probes

For `docs/research/whole-block-huge.md`. Nothing here is product code.

- `wbh_edit.py`, `wbh.rs`: the run-time switch (`ASMALLOC_WBH`) that gives a large block
  allocated whole the grown path's huge-page mark, counters (`WBH_DIAG`) and a map of the
  regions as they are assigned (`WBH_MAP`).
- `build.sh NAME COMMIT [PART...]`: builds the switch into the bench workloads, the preload
  library, the sparse probe, an app, or a benchrun patched by `runner_edit.py` (raw PMU
  events in the server counters), under `bench/target/NAME`. Run under `scripts/heavy`.
- `copy-incumbents.sh`: the incumbents' builds and the load generator from another checkout.
- `sparse/`, `sparse.py`: blocks allocated whole and touched sparsely, in a prefix or densely.
- `kern.py`, `pre.py`: the HashMap job and the screening's programs (tpchgen, jaq,
  rust-analyzer) by arm, with dTLB counters and sampled `AnonHugePages`; `wbhlib.py` holds
  what they share.
- `ibs.py`: IBS samples sorted by asmalloc region. `collapse.c`: `MADV_COLLAPSE` on ranges
  advised against huge pages, and on sparse ones.
- `summ.py`, `runsumm.py`: medians of the drivers' and of benchrun's records.
- `sessions/`: the sessions of the note, run from the checkout under `scripts/measure` (or
  `scripts/heavy` for `attrib.sh` and `smoke.sh`). The benchrun sessions need local
  `bench/allocators.toml` entries (not committed): `wbh-main`, `wbh-all2`, `wbh-nz2`,
  `wbh-all16` (`build = "wbh"`, `env = { ASMALLOC_WBH = "m0f002d0" ... , BENCH_EVENTS =
  "walk=0xf045" }`), `mi-t` and `tc-t` with the same variables, and `wbh-map` for the
  attribution runs (`WBH_MAP` and `WBH_DIAG` pointing into the output directory).
