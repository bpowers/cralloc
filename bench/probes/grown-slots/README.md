# grown-slots probes

For `docs/research/grown-slots.md`. Nothing here is product code.

- `grown_edit.py`, `gs.rs`: the run-time switch (`ASMALLOC_GROWN`) over the rule for grown
  blocks' slots, and counters of what returned each large slot and what later took it back.
- `build.sh NAME COMMIT [PART...]`: builds the switch into the bench workloads, the preload
  library, the trail probe or an app, under `bench/target/NAME`. Run under `scripts/heavy`.
- `trail/`: many buffers growing at once on a heap with live ballast; the trail each rule
  leaves.
- `kern.py`, `ksumm.py`: interleaved kernel-suite workloads by arm, and their medians.
- `tpch.py`: tpchgen from the screening's tree under the preload library, by arm.
- `appsumm.py`: benchrun records of the apps job by allocator. Server sessions are read with
  `../meili-peak/thread.py`.
- `diag.py`: the counters, as faults by what returned the memory and how long before.
- `sessions/`: the sessions of the note, each run from the checkout under `scripts/measure`.
  The app and server sessions need local `bench/allocators.toml` entries `gs-<arm>` with
  `build = "gs"` and `env = { ASMALLOC_GROWN = "<setting>" }` (not committed); the settings
  are in the note.
