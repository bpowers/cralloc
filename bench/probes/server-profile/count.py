#!/usr/bin/env python3
"""One benchrun run of a server job with the counting build (count_edit.py, build-count.sh), read
from outside during its fixed-rate segment.

  count.py --job "orders axum" --out DIR [--window S] [--tail S]

It reads the allocator's counters (ASMALLOC_PROF) and the wrapper's (BENCH_PROF_CALLS) from
/proc/PID/mem at two moments of the 70% segment, which gives counts per request over the
interval between them (the rate is fixed, so requests are rate times time). Then it sets the
wrapper's BENCH_PROF_STATE to 1 for WINDOW seconds (every call recorded) and to 2 for TAIL
seconds (frees of one block in 64 by address, and every large call), then to 0. The server
writes its record at exit into the allocator entry's BENCH_PROF_DIR (bench/target/prof/trace),
from where it is moved into DIR.

Run from the repository root under `scripts/measure --both-l3`; the counts are not timed, but
the run uses the measurement CPUs. Output in DIR: run.jsonl, run.log, counters.json,
trace-PID.bin, meta.json.
"""
import argparse
import json
import os
import struct
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from record import BENCH, CAP, JOBS, find
from where import RATES

ROW, ROWS = 64, 65


def symbols(binary, names):
    out = {}
    for line in subprocess.run(["nm", binary], capture_output=True, text=True).stdout.splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[2] in names:
            out[parts[2]] = int(parts[0], 16)
    return out


def base_of(pid, binary):
    for line in Path(f"/proc/{pid}/maps").read_text().splitlines():
        parts = line.split()
        if len(parts) >= 6 and parts[5] == binary and int(parts[2], 16) == 0:
            return int(parts[0].split("-")[0], 16)
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--job", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--alloc", default="asmalloc-prof")
    ap.add_argument("--window", type=float)
    ap.add_argument("--tail", type=float)
    a = ap.parse_args()
    suite, program, s70, s70_len = JOBS[a.job]
    large = suite == "servers-large"
    out = Path(a.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    build = "asmalloc-prof"
    server = str(BENCH / "target" / build / ("apps/" + program + "/release/" + program if program else "release/rpc-server"))
    loadgen = str(BENCH / "target/loadgen/release/loadgen")
    t_a = s70 + (2 if large else 0.5)
    t_b = t_a + (10 if large else 4)
    # Short enough that no thread fills its chunk of the record (4M calls).
    window = a.window or (0.2 if a.job == "meilisearch hn" else 0.1 if a.job == "connect store 1M" else 0.05)
    tail = a.tail or (28 if large else 1.0)
    before = {int(d) for d in os.listdir("/proc") if d.isdigit()}
    cmd = [str(BENCH / "target/release/benchrun"), "run", "--suite", suite, "--only", f"={a.job}",
           "--allocators", a.alloc, "--rounds", "1", "--out", str(out / "run.jsonl")]
    if a.job in CAP:
        cmd += ["--mem-cap", CAP[a.job]]
    runner = subprocess.Popen(cmd, cwd=BENCH, stdout=open(out / "run.log", "w"), stderr=subprocess.STDOUT)
    spid = gpid = t0 = None
    while runner.poll() is None:
        spid = spid or find(server, before)
        if gpid is None:
            gpid = find(loadgen, before)
            if gpid:
                t0 = time.monotonic()
        if spid and gpid:
            break
        time.sleep(0.005)
    if not (spid and gpid):
        sys.exit("no server or generator seen")
    sym = symbols(server, {"ASMALLOC_PROF", "BENCH_PROF_CALLS", "BENCH_PROF_STATE"})
    base = base_of(spid, server)
    mem = open(f"/proc/{spid}/mem", "r+b", buffering=0)

    def read(name, n):
        mem.seek(base + sym[name])
        return list(struct.unpack(f"<{n}Q", mem.read(8 * n)))

    def snapshot():
        return {"t": time.monotonic() - t0, "prof": read("ASMALLOC_PROF", ROW * ROWS),
                "calls": read("BENCH_PROF_CALLS", 64 * 8)}

    def set_state(v):
        mem.seek(base + sym["BENCH_PROF_STATE"])
        mem.write(struct.pack("<Q", v))

    def at(t):
        while time.monotonic() < t0 + t and runner.poll() is None:
            time.sleep(0.001)

    at(t_a)
    s1 = snapshot()
    at(t_b)
    s2 = snapshot()
    set_state(1)
    w0 = time.monotonic() - t0
    at(t_b + window)
    set_state(2)
    w1 = time.monotonic() - t0
    at(t_b + window + tail)
    set_state(0)
    w2 = time.monotonic() - t0
    s3 = snapshot()
    mem.close()
    runner.wait()
    # The allocator entry's BENCH_PROF_DIR is one directory for every run; the record moves here.
    trace = BENCH / "target/prof/trace" / f"trace-{spid}.bin"
    if trace.exists():
        trace.rename(out / trace.name)
    meta = {"job": a.job, "server": server, "server_pid": spid, "rate": RATES[a.job], "window": [w0, w1],
            "tail_end": w2, "runner_status": runner.returncode}
    (out / "counters.json").write_text(json.dumps({"s1": s1, "s2": s2, "s3": s3}))
    (out / "meta.json").write_text(json.dumps(meta, indent=1))
    for line in open(out / "run.log"):
        if "server cycles" in line or "invalid" in line.lower():
            print(line.rstrip())


if __name__ == "__main__":
    main()
