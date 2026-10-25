#!/usr/bin/env python3
"""One benchrun run of one server job with one allocator, profiled from outside during its
fixed-rate segment (docs/research/server-profile.md).

  record.py --job "orders axum" --alloc asmalloc --mode cycles|ibs|none --out DIR
            [--at SECONDS] [--dur SECONDS] [--period N] [--events name=cfg,...]

The run is `benchrun run --suite SUITE --only "=JOB" --allocators ALLOC --rounds 1`, so the
server is started, pinned, capped and loaded exactly as in the suite. The script watches /proc
for the load generator's start (where the plan's clock begins) and the server's pid (by its
executable), then at AT seconds into the plan records CPUs 0-3 for DUR seconds:

- `cycles`: `perf record -e cycles -c PERIOD -g -j any,u`: user and kernel time by
  instruction, with the kernel's call chain and the last 16 taken user branches, which name
  the caller of a leaf such as memcpy or a system call's wrapper.
- `ibs`: IBS op sampling (`ibs_op/cnt_ctl=1/`, one sample every PERIOD ops) with data
  addresses, and every page fault with its address: misses and TLB misses by data address.
- `sched`: every context switch on those CPUs (the `context-switches` software event, one
  sample each, and the switch records) with the kernel's call chain of the task switched out:
  where threads block and for how long (`offcpu.py`).

It snapshots the server's /proc/PID/maps during the recording, and in the first silence of the
plan the stack pointer of every thread (from /proc/PID/task/TID/syscall), so that addresses can
be told apart as heap, stack or other. `--events` passes raw PMU events to the runner
(`BENCH_PROC_EVENTS`, the runner patch in this directory), counted per request per segment.

Run it from the repository root under `scripts/measure --both-l3`. Output in DIR: run.jsonl and
run.log (benchrun's), perf.data, maps.txt, stacks.json, meta.json.
"""
import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

W = Path(__file__).resolve().parents[3]
BENCH = W / "bench"

# job: (suite, program or None for rpc-server, start of s70 in s, its length in s)
JOBS = {
    "orders axum": ("servers", "orders", 14, 6),
    "orders axum h2": ("servers", "orders", 14, 6),
    "connect log": ("servers", None, 14, 6),
    "connect log h2": ("servers", None, 14, 6),
    "connect log grpc h2": ("servers", None, 14, 6),
    "router graphql": ("servers", "router", 14, 6),
    # large-heap plans: _fill fill_s, sat 4, _w70 2, then six ten-second s70 pieces
    "orders axum cache 2G": ("servers-large", "orders", 6 + 6, 60),
    "connect store 1M": ("servers-large", None, 3 + 6, 60),
    "meilisearch hn": ("servers-large", "meilisearch", 5 + 6, 60),
    "qdrant search": ("servers-large", "qdrant", 5 + 6, 60),
    "qdrant mixed": ("servers-large", "qdrant", 5 + 6, 60),
    # saturation only: _fill 5, sat 5
    "qdrant mixed sat": ("servers-large-sat", "qdrant", 5, 5),
    "surrealdb mem": ("servers-large", "surrealdb", 5 + 6, 60),
}
BUILD = {"asmalloc": "asmalloc", "jemalloc": "je", "glibc": "glibc", "tcmalloc-percpu": "tc", "mimalloc": "mi",
         "asmalloc-dbg": "asmalloc-dbg",
         "jemalloc-dbg": "je-dbg"}
CAP = {"orders axum cache 2G": "7G", "connect store 1M": "8G", "meilisearch hn": "12G", "qdrant search": "12G",
       "qdrant mixed": "12G", "qdrant mixed sat": "12G", "surrealdb mem": "10G"}


def exe_of(pid):
    try:
        return os.readlink(f"/proc/{pid}/exe")
    except OSError:
        return None


def find(exe, newer_than=None):
    for d in os.listdir("/proc"):
        if d.isdigit() and exe_of(d) == exe:
            if newer_than is None or int(d) not in newer_than:
                return int(d)
    return None


def stacks(pid):
    """{tid: (comm, sp)} for threads blocked in a system call."""
    out = {}
    for tid in os.listdir(f"/proc/{pid}/task"):
        try:
            comm = Path(f"/proc/{pid}/task/{tid}/comm").read_text().strip()
            sc = Path(f"/proc/{pid}/task/{tid}/syscall").read_text().split()
        except OSError:
            continue
        if len(sc) >= 3 and sc[0] not in ("running", "-1"):
            out[tid] = (comm, int(sc[-2], 16))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--job", required=True)
    ap.add_argument("--alloc", required=True)
    ap.add_argument("--mode", choices=["cycles", "ibs", "sched", "none"], required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--at", type=float)
    ap.add_argument("--dur", type=float)
    ap.add_argument("--period", type=int)
    ap.add_argument("--events", default="")
    ap.add_argument("--build", help="benchrun build name, if not the allocator's usual one")
    a = ap.parse_args()
    suite, program, s70, s70_len = JOBS[a.job]
    at = a.at if a.at is not None else s70 + 0.5
    dur = a.dur if a.dur is not None else min(5.0, s70_len - 1.0)
    out = Path(a.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    build = a.build or BUILD[a.alloc]
    if program:
        server = BENCH / "target" / build / "apps" / program / "release" / program
    else:
        server = BENCH / "target" / build / "release" / "rpc-server"
    loadgen = BENCH / "target" / "loadgen" / "release" / "loadgen"
    before = {int(d) for d in os.listdir("/proc") if d.isdigit()}
    env = dict(os.environ)
    if a.events:
        env["BENCH_PROC_EVENTS"] = a.events
    cmd = [str(BENCH / "target/release/benchrun"), "run", "--suite", suite, "--only", f"={a.job}",
           "--allocators", a.alloc, "--rounds", "1", "--out", str(out / "run.jsonl")]
    if a.job in CAP:
        cmd += ["--mem-cap", CAP[a.job]]
    log = open(out / "run.log", "w")
    runner = subprocess.Popen(cmd, cwd=BENCH, stdout=log, stderr=subprocess.STDOUT, env=env)
    meta = {"job": a.job, "alloc": a.alloc, "mode": a.mode, "server": str(server), "at": at, "dur": dur}
    spid = gpid = None
    t0 = None
    deadline = time.time() + 1200
    while time.time() < deadline and runner.poll() is None:
        if spid is None:
            spid = find(str(server), before)
        if gpid is None:
            gpid = find(str(loadgen), before)
            if gpid:
                t0 = time.monotonic()
        if spid and gpid:
            break
        time.sleep(0.005)
    if not (spid and gpid):
        print("no server or generator seen", file=sys.stderr)
        runner.wait()
        sys.exit(1)
    meta.update(server_pid=spid, gen_pid=gpid)
    perf = None
    if a.mode != "none":
        while time.monotonic() < t0 + at:
            time.sleep(0.002)
        if a.mode == "cycles":
            period = a.period or 500_000
            # Kernel chains by frame pointer; the user side from the last 16 taken user branches
            # (AMD's LBR), which name the caller of a leaf such as memcpy or a syscall wrapper.
            # dwarf unwinding of these binaries failed for nine user samples in ten.
            ev = ["-e", "cycles", "-c", str(period), "-g", "-j", "any,u"]
        elif a.mode == "sched":
            period = 1
            # The software event, which needs no tracefs: a sample with the kernel's chain at
            # every switch out, and the switch records, which time each switch back in.
            ev = ["-e", "context-switches", "-c", "1", "-g", "--switch-events"]
        else:
            period = a.period or 500_000
            ev = ["-e", "ibs_op/cnt_ctl=1/", "-c", str(period), "-d", "-W", "--data-page-size"]
        meta["period"] = period
        meta["t_record"] = time.monotonic() - t0
        record = ["taskset", "-c", "13", "perf", "record", "-q", "-a", "-C", "0-3"]
        perf = subprocess.Popen([*record, "-o", str(out / "perf.data"), *ev, "--", "sleep", str(dur)],
                                stdout=subprocess.DEVNULL, stderr=open(out / "perf.err", "w"))
        faults = None
        if a.mode == "ibs":
            # Every page fault with its address, in a recording of its own (perf script cannot
            # print IBS's fields for a file that also holds software events).
            faults = subprocess.Popen([*record, "-o", str(out / "faults.data"), "-e", "page-faults", "-c", "1", "-d",
                                       "--", "sleep", str(dur)], stdout=subprocess.DEVNULL,
                                      stderr=open(out / "faults.err", "w"))
        time.sleep(min(1.0, dur / 2))
        try:
            (out / "maps.txt").write_text(Path(f"/proc/{spid}/maps").read_text())
        except OSError:
            pass
        perf.wait()
        if faults:
            faults.wait()
    # The first silence of the plan: after s70, the first `off` segment (3 s).
    off_at = s70 + s70_len + 1.5
    while time.monotonic() < t0 + off_at and runner.poll() is None:
        time.sleep(0.01)
    try:
        st = stacks(spid)
        (out / "stacks.json").write_text(json.dumps(st))
        if not (out / "maps.txt").exists():
            (out / "maps.txt").write_text(Path(f"/proc/{spid}/maps").read_text())
    except OSError as e:
        meta["stacks_error"] = str(e)
    runner.wait()
    meta["runner_status"] = runner.returncode
    (out / "meta.json").write_text(json.dumps(meta, indent=1))
    for line in open(out / "run.log"):
        if "server cycles" in line or "invalid" in line.lower():
            print(line.rstrip())


if __name__ == "__main__":
    main()
