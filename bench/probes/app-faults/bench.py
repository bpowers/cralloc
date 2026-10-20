#!/usr/bin/env python3
"""Interleaved runs of real programs under several preload libraries.

  bench.py OUT.jsonl --rounds N --progs a,b --libs tag=path,tag=path [--glibc]

Each run: a 300 ms spin on its CPUs, then perf stat around /usr/bin/time around walltime.
A library given as "-" means glibc. DIAG lines on stderr (diagnostic builds) are kept.
A measurement: run under scripts/measure --both-l3 and scripts/memlimit.
"""
import os
import argparse
import json
import random
import subprocess
import time
from pathlib import Path

S = Path(os.environ.get("APPF_OUT", "/tmp/app-faults"))
EVENTS = "cycles:u,cycles:k,instructions:u,task-clock,page-faults,context-switches"
ST = {"pulldown-cmark"}


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


def diag(stderr):
    sites, calls = {}, {}
    for line in stderr.read_text(errors="replace").splitlines():
        f = line.split()
        if len(f) >= 9 and f[0] == "DIAG" and f[1] == "site":
            name = f[2] if f[2].startswith("walk") else f[2].rstrip("0123456789")
            key = f"{name} {f[3]}"
            sites[key] = {"calls": int(f[5]), "bytes": int(f[7]), "ticks": int(f[9])}
            if not name.startswith("walk"):
                calls[name] = calls.get(name, 0) + int(f[5])
        elif len(f) >= 4 and f[0] == "DIAG" and f[1] == "revived":
            calls[f"revived_{f[2]}"] = calls.get(f"revived_{f[2]}", 0) + int(f[-1])
    return sites, calls


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--rounds", type=int, default=5)
    ap.add_argument("--progs", required=True)
    ap.add_argument("--libs", required=True)
    a = ap.parse_args()
    libs = dict(x.split("=", 1) for x in a.libs.split(","))
    progs = a.progs.split(",")
    tmp = S / "benchtmp"
    tmp.mkdir(exist_ok=True)
    for r in range(a.rounds):
        for prog in progs:
            order = list(libs)
            random.shuffle(order)
            for tag in order:
                cpus = "2" if prog in ST else "0-3,8-11"
                stat, wall, rss = tmp / "stat", tmp / "wall", tmp / "rss"
                for p in (stat, wall, rss):
                    p.unlink(missing_ok=True)
                subprocess.run([str(Path(os.environ["RUST_SCREEN_ROOT"]) / "tools/spin"), cpus, "300"], check=True)
                argv = [str(Path(__file__).resolve().parent / "run.sh"), prog, libs[tag], "perf", "stat", "-x,", "-o", str(stat), "-e", EVENTS,
                        "--", "/usr/bin/time", "-f", "%M", "-o", str(rss), "taskset", "-c", cpus,
                        str(Path(os.environ["RUST_SCREEN_ROOT"]) / "tools/walltime"), str(wall)]
                p = subprocess.run(argv, timeout=600)
                rec = {"prog": prog, "lib": tag, "round": r, "t": time.time(), "rc": p.returncode,
                       "wall": float(wall.read_text()), "rss_kib": int(rss.read_text().split()[-1]),
                       "stat": parse_stat(stat)}
                rec["sites"], rec["calls"] = diag(S / "work" / prog / "stderr")
                with open(a.out, "a") as f:
                    f.write(json.dumps(rec) + "\n")
                st = rec["stat"]
                print(f"{prog:15} {tag:10} r{r} wall {rec['wall']:.3f} cyc {((st.get('cycles:u') or 0) + (st.get('cycles:k') or 0)) / 1e9:.2f}G "
                      f"k {(st.get('cycles:k') or 0) / 1e9:.2f}G cpu {(st.get('task-clock') or 0) / 1e3:.2f}s "
                      f"pf {int(st.get('page-faults') or 0)} cs {int(st.get('context-switches') or 0)} "
                      f"rss {rec['rss_kib'] // 1024}M {rec['calls']}", flush=True)


if __name__ == "__main__":
    main()
