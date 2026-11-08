#!/usr/bin/env python3
"""Raw counters of jobs under two or more allocators, to see what a gap between them is made of:
events.py --builds cralloc,mi [--runs N] [--cpu CPU] [--mem SIZE] EVENTS JOB...

EVENTS is what BENCH_EVENTS takes, at most three on a core whose counters are not interchangeable, and
they count user mode only (firestorm.md and hardware.md list the codes). JOB is `workload` arguments in
one word, spaces as commas: `json,size=32Mi,reps=5`. A build is a directory under bench/target. Prints,
per build, the median over runs of each event, cycles, instructions and faults, summed over the
workload's measured phases, and the ratio of each to the first build's. Run it under scripts/measure;
each run goes through scripts/memlimit."""
import argparse
import json
import os
import statistics
import subprocess
import sys

BENCH = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def run(build, job, events, cpu, mem):
    env = dict(os.environ, BENCH_EVENTS=events, BENCH_CPUS=str(cpu), MEMLIMIT_QUIET="1")
    cmd = [
        os.path.join(BENCH, "../scripts/memlimit"), "-m", mem, "-t", "5m", "--",
        "taskset", "-c", str(cpu), os.path.join(BENCH, "target", build, "release/workload"), *job.split(","),
    ]
    out = subprocess.run(cmd, env=env, capture_output=True, text=True, check=True).stdout
    line = next(l for l in out.splitlines() if l.startswith("BENCH_RESULT "))
    result = json.loads(line.removeprefix("BENCH_RESULT "))
    totals = {}
    for phase in result["phases"].values():
        if phase.get("multiplexed"):
            raise SystemExit(f"{build} {job}: counters were multiplexed; name fewer events")
        for k, v in phase.items():
            if type(v) is int and k not in ("wall_ns", "task_clock_ns", "ctx_switches"):
                totals[k] = totals.get(k, 0) + v
    return totals


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--builds", required=True)
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--cpu", type=int, default=8)
    ap.add_argument("--mem", default="4G")
    ap.add_argument("events")
    ap.add_argument("jobs", nargs="+")
    a = ap.parse_args()
    builds = a.builds.split(",")
    for job in a.jobs:
        per = {b: [] for b in builds}
        for _ in range(a.runs):
            for b in builds:
                per[b].append(run(b, job, a.events, a.cpu, a.mem))
        print(f"\n{job}  (median of {a.runs}; ratio to {builds[0]})")
        print(f"{'':24}" + "".join(f"{b:>22}" for b in builds))
        for k in sorted(per[builds[0]][0]):
            meds = [statistics.median(r[k] for r in per[b]) for b in builds]
            cells = [f"{m:14.4g} {m / meds[0] if meds[0] else 0:6.3f} " for m in meds]
            print(f"{k:24}" + "".join(f"{c:>22}" for c in cells))
        sys.stdout.flush()


main()
