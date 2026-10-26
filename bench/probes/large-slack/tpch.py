#!/usr/bin/env python3
"""tpchgen (TPC-H SF1 to Parquet, eight threads) from the screening's tree under the preload
library, one arm a setting of the switch, interleaved, for docs/research/large-slack.md (copied from large-churn/tpch.py, the switch renamed). As
bench/probes/app-faults/bench.py, whose run.sh it uses: a 300 ms spin on its CPUs, then perf
stat around /usr/bin/time around walltime, each run under scripts/memlimit.

  tpch.py OUT.jsonl --rounds N --arms 'main=LIB:t0a0,glibc=-' [--prog tpchgen] [--diag]

RUST_SCREEN_ROOT names the screening's tree, APPF_OUT a work directory on disk (tpchgen writes
its Parquet there). A measurement: run under scripts/measure --both-l3.
"""
import argparse
import json
import os
import random
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
RUN = ROOT / "bench/probes/app-faults/run.sh"
EVENTS = "cycles:u,cycles:k,instructions:u,task-clock,page-faults,context-switches"


def parse_stat(path):
    vals = {}
    for line in Path(path).read_text().splitlines():
        f = line.split(",")
        if len(f) < 3 or line.startswith("#"):
            continue
        try:
            vals[f[2]] = float(f[0])
        except ValueError:
            vals[f[2]] = None
    return vals


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--rounds", type=int, default=5)
    ap.add_argument("--arms", required=True)
    ap.add_argument("--prog", default="tpchgen")
    ap.add_argument("--cpus", default="0-3,8-11")
    ap.add_argument("--diag", action="store_true")
    a = ap.parse_args()
    arms = {}
    for spec in a.arms.split(","):
        name, rest = spec.split("=", 1)
        lib, _, setting = rest.partition(":")
        arms[name] = (lib, setting or "t1a1")
    S = Path(os.environ["APPF_OUT"])
    R = Path(os.environ["RUST_SCREEN_ROOT"])
    tmp = S / "gs-tmp"
    tmp.mkdir(parents=True, exist_ok=True)
    for r in range(a.rounds):
        order = list(arms)
        random.shuffle(order)
        for name in order:
            lib, setting = arms[name]
            stat, wall, rss, dg = tmp / "stat", tmp / "wall", tmp / "rss", tmp / "diag"
            for p in (stat, wall, rss, dg):
                p.unlink(missing_ok=True)
            env = dict(os.environ, ASMALLOC_LSR=setting)
            env["ASMALLOC_LSR_OUT"] = str(dg)
            subprocess.run([str(R / "tools/spin"), a.cpus, "300"], check=True)
            argv = [str(ROOT / "scripts/memlimit"), "-m", "8G", "-t", "10m", "--", str(RUN), a.prog, lib,
                    "perf", "stat", "-x,", "-o", str(stat), "-e", EVENTS, "--", "/usr/bin/time", "-f", "%M",
                    "-o", str(rss), "taskset", "-c", a.cpus, str(R / "tools/walltime"), str(wall)]
            p = subprocess.run(argv, env=env, timeout=900, capture_output=True, text=True)
            rec = {"prog": a.prog, "lib": name, "setting": setting, "round": r, "t": time.time(), "rc": p.returncode,
                   "wall": float(wall.read_text()), "rss_kib": int(rss.read_text().split()[-1]),
                   "stat": parse_stat(stat), "memlimit": p.stderr.strip().splitlines()[-1:]}
            if dg.exists():
                rec["lsr"] = dg.read_text().strip()
            with open(a.out, "a") as f:
                f.write(json.dumps(rec) + "\n")
            st = rec["stat"]
            print(f"{a.prog} {name:8} r{r} wall {rec['wall']:.3f} cyc {((st.get('cycles:u') or 0) + (st.get('cycles:k') or 0)) / 1e9:.2f}G "
                  f"k {(st.get('cycles:k') or 0) / 1e9:.2f}G pf {int(st.get('page-faults') or 0)} "
                  f"rss {rec['rss_kib'] // 1024}M", flush=True)


if __name__ == "__main__":
    main()
