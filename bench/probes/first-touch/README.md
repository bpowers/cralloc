# first-touch probes

For `docs/research/first-touch.md`. Nothing here is product code.

**Part 1, what a populate costs against a fault.**
- `populate.c`: the C probe (`cost`, `hold`, `neigh`; usage in its header).
- `kernel.sh`: the session that ran all three, with the machine's huge-page state.

**Part 2, the address-space trace.** It times every call the allocator makes that takes the
address space's lock (`mprotect`, `madvise`, `munmap`, `mmap`, `mincore`). Each record holds
the thread, what the caller was doing, the thread's CPU time, and its voluntary and
involuntary context switches during the call. The records go into an exported ring in
memory, and a watcher copies them out once a second, so the server makes no extra system
call to report. The cost is about half a microsecond added to each call.
- `wl.rs`, `wl_edit.py`: the trace and the edit that puts it into a scratch tree.
  - The edit's anchors are exact text from main `49986cf`: the syscall helper `range_call` and
    `mincore`/`reserve` in `os/linux/kernel.rs`, plus the first lines of the functions it
    tags in `back/`.
  - It fails loudly on a mismatch. To apply it to a newer main, fix the anchors that fail.
    The tags are optional: drop a `Ctx::enter` line whose function is gone.
- `build.sh NAME COMMIT [PART...]`: builds the RPC servers and the order service and
  Meilisearch apps with the trace, into `bench/target/NAME`. Run it under `scripts/heavy`.
- `session.sh`: one benchrun job with the watcher (`wlwatch.py`) beside it, keeping every
  request's record. Run it from `bench/` under `scripts/measure --both-l3`. It needs a local
  `bench/allocators.toml` entry `asmalloc-wl` with `feature = "asmalloc"`, `build = "wl"`
  (not committed).
- `wlsum.py OUTDIR TAG`: calls by segment, operation, thread and what the caller was doing;
  the worst writers and what overlapped them; and the slowest 0.1% of requests against the
  long calls.
- `wllong.py OUTDIR TAG`: every long call, with its CPU time and switches, and purge
  durations by size.
- `census.sh`, `censum.py`: strace attached to the server for a few seconds under load, to
  count the server's own calls beside the allocator's. Its timings are not measurements.
