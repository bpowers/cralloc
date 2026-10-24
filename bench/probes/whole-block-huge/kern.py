#!/usr/bin/env python3
"""Interleaved runs of the kernel suite's workloads under arms of one binary (settings of
ASMALLOC_WBH) and other allocators' builds, with dTLB misses and page walks counted
(BENCH_EVENTS) and the process's huge-page-backed memory sampled, for
docs/research/whole-block-huge.md.

  kern.py OUT.jsonl --rounds N --jobs 'map kind=hash n=1M reps=5' \
      --arms 'main=wbh:m0f002d0,all2=wbh:m1f002d0,mi=/path/to/mi/workload'

An arm is NAME=BUILD:SETTING (bench/target/BUILD/release/workload of this checkout) or
NAME=PATH. Every run is pinned to CPU 2 under scripts/memlimit, with the same environment
length in every arm. Each record has the workload's result, the sampled AnonHugePages, the
machine's free 2 MiB blocks before the run and its thp_ counters over it. With --diag,
WBH_DIAG's file is added. A measurement: run it under scripts/measure.
"""
import argparse
import json
import os
import random
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import wbhlib  # noqa: E402

ROOT = Path(__file__).resolve().parents[3]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--jobs", required=True)
    ap.add_argument("--arms", required=True)
    ap.add_argument("--cpu", default="2")
    ap.add_argument("--mem", default="4G")
    ap.add_argument("--diag", action="store_true")
    a = ap.parse_args()
    arms = {}
    for spec in a.arms.split(","):
        name, rest = spec.split("=", 1)
        if ":" in rest and not rest.startswith("/"):
            build, setting = rest.split(":", 1)
            arms[name] = (str(ROOT / "bench/target" / build / "release/workload"), setting)
        else:
            arms[name] = (rest, None)
    jobs = [j.strip() for j in a.jobs.split(";") if j.strip()]
    work = Path(a.out).with_suffix(".work")
    work.mkdir(parents=True, exist_ok=True)
    diag, pidfile = work / "diag", work / "pid"
    for r in range(a.rounds):
        for job in jobs:
            order = list(arms)
            random.shuffle(order)
            for name in order:
                exe, setting = arms[name]
                env = dict(os.environ, BENCH_CPUS=a.cpu, BENCH_EVENTS=wbhlib.EVENTS)
                env["ASMALLOC_WBH"] = setting or "m0f002d0"
                diag.unlink(missing_ok=True)
                pidfile.unlink(missing_ok=True)
                if a.diag:
                    env["WBH_DIAG"] = str(diag)
                cmd = [str(ROOT / "scripts/memlimit"), "-m", a.mem, "-t", "10m", "--"] + \
                    wbhlib.pid_wrapper(pidfile) + ["taskset", "-c", a.cpu, exe] + job.split()
                free = wbhlib.free_huge()
                v0 = wbhlib.vmstat_thp()
                t0 = time.time()
                sampler = wbhlib.Sampler(pidfile, ms=100)
                p = subprocess.run(cmd, env=env, capture_output=True, text=True)
                smaps = sampler.finish()
                thp = wbhlib.delta(v0, wbhlib.vmstat_thp())
                res = None
                for line in p.stdout.splitlines():
                    if line.startswith("BENCH_RESULT "):
                        res = json.loads(line[len("BENCH_RESULT "):])
                peak = None
                for line in p.stderr.splitlines():
                    if "memory peak" in line:
                        peak = line.split("over")[-1].split(",")[-1].strip()
                rec = {"job": job, "arm": name, "setting": setting, "round": r, "t": t0, "rc": p.returncode,
                       "result": res, "memlimit": peak, "smaps": smaps, "free_2m": free, "thp": thp}
                if a.diag and diag.exists():
                    rec["diag"] = diag.read_text()
                with open(a.out, "a") as f:
                    f.write(json.dumps(rec) + "\n")
                ph = (res or {}).get("phases", {})
                tot = lambda k: sum(v.get(k, 0) for v in ph.values())  # noqa: E731
                print(f"r{r} {job[:30]:30} {name:7} rc={p.returncode} Mcyc {tot('cycles')/1e6:8.1f} "
                      f"pf {tot('page_faults'):7d} miss {tot('dtlb_miss')/1e6:6.2f}M w4k {tot('walk_4k')/1e6:6.2f}M "
                      f"w2m {tot('walk_2m')/1e6:6.2f}M hugepk {smaps.get('huge_peak', 0)/1024:6.1f} "
                      f"free2m {free} thpfa {thp.get('thp_fault_alloc', 0)} fb {thp.get('thp_fault_fallback', 0)} {peak}",
                      flush=True)


if __name__ == "__main__":
    main()
