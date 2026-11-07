#!/usr/bin/env python3
"""tpchgen (TPC-H SF1 to Parquet, eight threads) under each allocator's preload library,
interleaved: a 300 ms spin on its CPUs, then perf stat around /usr/bin/time around the program,
each run under scripts/memlimit. tcmalloc gets the environment its entry in allocators.toml
gives it: glibc's rseq registration off, so that it runs with per-CPU caches.

  tpch.py OUT.jsonl WORKDIR [--rounds N] [--cpus LIST]

The program is the tpchgen-cli that `bench/apps/fetch datafusion` installs, built with the
system allocator, so that the preload takes every allocation. `benchrun build` makes cralloc's
preload library. WORKDIR must be on disk: tpchgen writes its Parquet there. A measurement: run
under scripts/measure --both-l3.
"""
import argparse
import json
import os
import random
import shutil
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
COMMON = Path(subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--path-format=absolute", "--git-common-dir"],
                             capture_output=True, text=True, check=True).stdout.strip()).parent
BASE = Path(os.environ.get("BENCH_BASELINES", COMMON / "bench/baselines/out"))
DATA = Path(os.environ.get("BENCH_APPS_DATA", COMMON / "bench/apps/target"))
TPCHGEN = DATA / "inputs/datafusion/tool/bin/tpchgen-cli"
EVENTS = "cycles:u,cycles:k,instructions:u,task-clock,page-faults,context-switches"
ARMS = {
    "cralloc": (str(ROOT / "target/release/libcralloc_preload.so"), {}),
    "jemalloc": (str(BASE / "jemalloc/libjemalloc.so"), {}),
    "tcmalloc-percpu": (str(BASE / "tcmalloc/libtcmalloc.so"), {"GLIBC_TUNABLES": "glibc.pthread.rseq=0"}),
    "mimalloc": (str(BASE / "mimalloc/libmimalloc.so"), {}),
    "glibc": (None, {}),
}


def cpu_list(text):
    cpus = []
    for part in text.split(","):
        lo, _, hi = part.partition("-")
        cpus.extend(range(int(lo), int(hi or lo) + 1))
    return cpus


def spin(cpus, ms):
    """One busy process on each CPU, so that the run finds the package at its boost clock
    rather than ramping from idle (bench/README.md, "Clock")."""
    end = time.monotonic() + ms / 1e3
    kids = []
    for cpu in cpus:
        pid = os.fork()
        if pid == 0:
            os.sched_setaffinity(0, {cpu})
            while time.monotonic() < end:
                pass
            os._exit(0)
        kids.append(pid)
    for pid in kids:
        os.waitpid(pid, 0)


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
    ap.add_argument("workdir")
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--cpus", default="0-3,8-11")
    a = ap.parse_args()
    work = Path(a.workdir).resolve()
    work.mkdir(parents=True, exist_ok=True)
    cpus = cpu_list(a.cpus)
    for r in range(a.rounds):
        order = list(ARMS)
        random.shuffle(order)
        for name in order:
            lib, extra = ARMS[name]
            stat, times = work / "stat", work / "times"
            for p in (stat, times):
                p.unlink(missing_ok=True)
            shutil.rmtree(work / "out", ignore_errors=True)
            spin(cpus, 300)
            preload = [f"LD_PRELOAD={lib}"] if lib else []
            argv = [str(ROOT / "scripts/memlimit"), "-m", "8G", "-t", "10m", "--",
                    "perf", "stat", "-x,", "-o", str(stat), "-e", EVENTS, "--",
                    "/usr/bin/time", "-f", "%e %M", "-o", str(times), "taskset", "-c", a.cpus,
                    "env", *preload, str(TPCHGEN), "parquet", "-s", "1", "--output-dir=out"]
            p = subprocess.run(argv, env=dict(os.environ, **extra), cwd=work, timeout=900,
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            # time prints a line of its own first when the command fails.
            wall, rss = times.read_text().split()[-2:]
            rec = {"prog": "tpchgen", "lib": name, "round": r, "t": time.time(), "rc": p.returncode,
                   "wall": float(wall), "rss_kib": int(rss), "stat": parse_stat(stat),
                   "memlimit": p.stderr.strip().splitlines()[-1:]}
            with open(a.out, "a") as f:
                f.write(json.dumps(rec) + "\n")
            st = rec["stat"]
            print(f"tpchgen {name:16} r{r} rc {p.returncode} wall {rec['wall']:.2f} cyc "
                  f"{((st.get('cycles:u') or 0) + (st.get('cycles:k') or 0)) / 1e9:.2f}G pf {int(st.get('page-faults') or 0)} "
                  f"rss {rec['rss_kib'] // 1024}M", flush=True)


if __name__ == "__main__":
    main()
