#!/usr/bin/env python3
"""Interleaved runs of the kernel suite's workloads under arms of one binary (the switch's
settings) and other allocators' builds, for docs/research/grown-slots.md.

  kern.py OUT.jsonl --rounds N --jobs 'vecgrow vecs=64 max=4Mi reps=4;map kind=hash n=1M reps=5' \
      --arms 'main=gs:m0b0016s00r0d0,keep=gs:m2b0016s00r0d0,je=/path/to/je/workload'

An arm is NAME=BUILD:SETTING, BUILD being bench/target/BUILD/release/workload of this
checkout and SETTING the value of ASMALLOC_GROWN, or NAME=PATH for another allocator's
workload binary. Every run is pinned to CPU 2 (BENCH_CPUS) under scripts/memlimit. With
--diag, GS_DIAG names a file per run whose line is added to the record. A measurement: run it
under scripts/measure.
"""
import argparse
import json
import os
import random
import subprocess
import time
from pathlib import Path

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
    tmp = Path(a.out).with_suffix(".diag")
    for r in range(a.rounds):
        for job in jobs:
            order = list(arms)
            random.shuffle(order)
            for name in order:
                exe, setting = arms[name]
                env = dict(os.environ, BENCH_CPUS=a.cpu)
                # The same length in every arm, so that the stack starts alike.
                env["ASMALLOC_GROWN"] = setting or "m0b0016s00r0d0"
                if a.diag:
                    tmp.unlink(missing_ok=True)
                    env["GS_DIAG"] = str(tmp)
                cmd = [str(ROOT / "scripts/memlimit"), "-m", a.mem, "-t", "10m", "--",
                       "taskset", "-c", a.cpu, exe] + job.split()
                t0 = time.time()
                p = subprocess.run(cmd, env=env, capture_output=True, text=True)
                res = None
                for line in p.stdout.splitlines():
                    if line.startswith("BENCH_RESULT "):
                        res = json.loads(line[len("BENCH_RESULT "):])
                peak = None
                for line in p.stderr.splitlines():
                    if "memory peak" in line:
                        peak = line.split("over")[-1].split(",")[-1].strip()
                rec = {"job": job, "arm": name, "round": r, "t": t0, "rc": p.returncode,
                       "result": res, "memlimit": peak}
                if a.diag and tmp.exists():
                    rec["diag"] = tmp.read_text().strip()
                with open(a.out, "a") as f:
                    f.write(json.dumps(rec) + "\n")
                ph = (res or {}).get("phases", {})
                cyc = sum(v.get("cycles", 0) for v in ph.values())
                cyu = sum(v.get("cycles_user", 0) for v in ph.values())
                pf = sum(v.get("page_faults", 0) for v in ph.values())
                hwm = (res or {}).get("vm_hwm", 0) / 2**20
                print(f"r{r} {job[:34]:34} {name:8} rc={p.returncode} Mcyc {cyc/1e6:9.1f} user {cyu/1e6:8.1f} "
                      f"pf {pf:8d} hwm {hwm:7.1f} {peak}", flush=True)


if __name__ == "__main__":
    main()
