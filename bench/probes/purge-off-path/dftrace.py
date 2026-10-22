#!/usr/bin/env python3
"""DataFusion runs with a trace of the allocator's thread (docs/research/purge-off-path.md):
every 5 ms, the process's RssAnon and the thread's CPU time, run-queue wait and voluntary
switches (its wake-ups), from /proc. Prints each run's peak and writes the traces.

  dftrace.py BINARY DATA EXPECT --runs N --out DIR

Run under scripts/measure --both-l3; the program runs pinned to CPUs 0-3 under
scripts/memlimit, as the apps suite runs it.
"""
import argparse
import json
import os
import subprocess
import time
from pathlib import Path

W = Path(__file__).resolve().parents[3]


def thread_dir(pid):
    try:
        for t in os.listdir(f"/proc/{pid}/task"):
            try:
                if open(f"/proc/{pid}/task/{t}/comm").read().strip() == "asmalloc":
                    return f"/proc/{pid}/task/{t}"
            except OSError:
                pass
    except OSError:
        pass
    return None


def field(path, name):
    for line in open(path):
        if line.startswith(name):
            return int(line.split()[1])
    return None


def main():
    p = argparse.ArgumentParser()
    p.add_argument("binary")
    p.add_argument("data")
    p.add_argument("expect")
    p.add_argument("--runs", type=int, default=10)
    p.add_argument("--out", required=True)
    a = p.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    for n in range(a.runs):
        cmd = [str(W / "scripts/memlimit"), "-m", "4G", "-t", "2m", "--", "taskset", "-c", "0-3",
               a.binary, "tpch", f"data={a.data}", f"expect={a.expect}"]
        proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                env=dict(os.environ, MEMLIMIT_QUIET="1"))
        pid, tdir, trace, start = None, None, [], time.monotonic()
        while proc.poll() is None:
            if pid is None:
                r = subprocess.run(["pgrep", "-n", "-f", f"^{a.binary} tpch"], capture_output=True, text=True)
                pid = int(r.stdout.split()[0]) if r.stdout.split() else None
            if pid is not None:
                try:
                    rss = field(f"/proc/{pid}/status", "RssAnon:")
                    if tdir is None:
                        tdir = thread_dir(pid)
                    run_ns = wait_ns = vol = None
                    if tdir:
                        run_ns, wait_ns, _ = (int(x) for x in open(f"{tdir}/schedstat").read().split())
                        vol = field(f"{tdir}/status", "voluntary_ctxt_switches:")
                    trace.append([round(time.monotonic() - start, 4), rss, run_ns, wait_ns, vol])
                except OSError:
                    pass
            time.sleep(0.005)
        peak = max((t[1] or 0) for t in trace) >> 10 if trace else 0
        (out / f"run{n}.json").write_text(json.dumps(trace))
        print(f"run {n}: peak RssAnon {peak} MiB, samples {len(trace)}, status {proc.returncode}", flush=True)


main()
