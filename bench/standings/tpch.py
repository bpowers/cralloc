#!/usr/bin/env python3
"""tpchgen (TPC-H SF1 to Parquet, eight threads) from the screening's tree under each
allocator's preload library, interleaved, for the standings. As large-slack/tpch.py, which it
follows (a 300 ms spin on its CPUs, then perf stat around /usr/bin/time around walltime, each
run under scripts/memlimit), with each allocator's environment from allocators.toml: tcmalloc
gets glibc's rseq registration turned off, so that it runs with per-CPU caches.

  tpch.py OUT.jsonl --rounds N

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
BASE = Path(os.environ.get("BENCH_BASELINES", "/home/bpowers/src/asmalloc/bench/baselines/out"))
EVENTS = "cycles:u,cycles:k,instructions:u,task-clock,page-faults,context-switches"
ARMS = {
    "asmalloc": (str(ROOT / "target/release/libasmalloc_preload.so"), {}),
    "jemalloc": (str(BASE / "jemalloc/libjemalloc.so"), {}),
    "tcmalloc-percpu": (str(BASE / "tcmalloc/libtcmalloc.so"), {"GLIBC_TUNABLES": "glibc.pthread.rseq=0"}),
    "mimalloc": (str(BASE / "mimalloc/libmimalloc.so"), {}),
    "glibc": ("-", {}),
}


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
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--cpus", default="0-3,8-11")
    a = ap.parse_args()
    S = Path(os.environ["APPF_OUT"])
    R = Path(os.environ["RUST_SCREEN_ROOT"])
    tmp = S / "st-tmp"
    tmp.mkdir(parents=True, exist_ok=True)
    for r in range(a.rounds):
        order = list(ARMS)
        random.shuffle(order)
        for name in order:
            lib, extra = ARMS[name]
            stat, wall, rss = tmp / "stat", tmp / "wall", tmp / "rss"
            for p in (stat, wall, rss):
                p.unlink(missing_ok=True)
            env = dict(os.environ, **extra)
            subprocess.run([str(R / "tools/spin"), a.cpus, "300"], check=True)
            argv = [str(ROOT / "scripts/memlimit"), "-m", "8G", "-t", "10m", "--", str(RUN), "tpchgen", lib,
                    "perf", "stat", "-x,", "-o", str(stat), "-e", EVENTS, "--", "/usr/bin/time", "-f", "%M",
                    "-o", str(rss), "taskset", "-c", a.cpus, str(R / "tools/walltime"), str(wall)]
            p = subprocess.run(argv, env=env, timeout=900, capture_output=True, text=True)
            rec = {"prog": "tpchgen", "lib": name, "round": r, "t": time.time(), "rc": p.returncode,
                   "wall": float(wall.read_text()), "rss_kib": int(rss.read_text().split()[-1]),
                   "stat": parse_stat(stat), "memlimit": p.stderr.strip().splitlines()[-1:]}
            with open(a.out, "a") as f:
                f.write(json.dumps(rec) + "\n")
            st = rec["stat"]
            print(f"tpchgen {name:16} r{r} rc {p.returncode} wall {rec['wall']:.3f} cyc "
                  f"{((st.get('cycles:u') or 0) + (st.get('cycles:k') or 0)) / 1e9:.2f}G pf {int(st.get('page-faults') or 0)} "
                  f"rss {rec['rss_kib'] // 1024}M", flush=True)


if __name__ == "__main__":
    main()
