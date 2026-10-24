#!/usr/bin/env python3
"""The sparse-tenant probe (sparse/) under the switch's arms, interleaved, on one CPU, for
docs/research/whole-block-huge.md: what a block allocated whole costs in time and memory when
huge pages are wrong for it, and what they buy when they are right.

  sparse.py OUT.jsonl --rounds N --arms 'main=m0f002d0,all2=m1f002d0' [--cases NAME,...]

Every run is bench/target/wbh/sparse pinned to CPU 2 under scripts/memlimit. Each record is
one run: the case, the arm, and the probe's lines (the first life is a warm-up). A
measurement: run it under scripts/measure.
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
EXE = ROOT / "bench/target/wbh/sparse"

CASES = {
    "hash64-1/64": "case=random size=64 blocks=4 touch=64 lives=6",
    "hash64-1/16": "case=random size=64 blocks=4 touch=16 lives=6",
    "hash64-all": "case=random size=64 blocks=4 touch=1 lives=6",
    "hash16-1/64": "case=random size=16 blocks=8 touch=64 lives=6",
    "prefix64-1M": "case=prefix size=64 blocks=4 touch=1024 lives=6",
    "prefix16-256K": "case=prefix size=16 blocks=8 touch=256 lives=6",
    "zhash64-1/64": "case=random size=64 blocks=4 touch=64 lives=6 zero=1",
    "dense64-reads": "case=dense size=64 blocks=4 lives=6 reads=2000000",
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--arms", required=True)
    ap.add_argument("--cases", default=",".join(CASES))
    ap.add_argument("--cpu", default="2")
    a = ap.parse_args()
    arms = dict(s.split("=", 1) for s in a.arms.split(","))
    cases = a.cases.split(",")
    for r in range(a.rounds):
        for case in cases:
            order = list(arms)
            random.shuffle(order)
            for name in order:
                env = dict(os.environ, ASMALLOC_WBH=arms[name])
                cmd = [str(ROOT / "scripts/memlimit"), "-m", "2G", "-t", "5m", "--",
                       "taskset", "-c", a.cpu, str(EXE)] + CASES[case].split()
                free = wbhlib.free_huge()
                v0 = wbhlib.vmstat_thp()
                t0 = time.time()
                p = subprocess.run(cmd, env=env, capture_output=True, text=True)
                thp = wbhlib.delta(v0, wbhlib.vmstat_thp())
                lines = [ln for ln in p.stdout.splitlines() if ln.startswith(("life", "end"))]
                rec = {"case": case, "arm": name, "setting": arms[name], "round": r, "t": t0, "rc": p.returncode,
                       "free_2m": free, "thp": thp, "lines": lines}
                with open(a.out, "a") as f:
                    f.write(json.dumps(rec) + "\n")
                last = lines[-2] if len(lines) >= 2 else ""
                print(f"r{r} {case:14} {name:6} rc={p.returncode} free2m {free} stall {thp.get('compact_stall', 0)} "
                      f"fb {thp.get('thp_fault_fallback', 0)} | {last}", flush=True)


if __name__ == "__main__":
    main()
