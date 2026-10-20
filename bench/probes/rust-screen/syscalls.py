#!/usr/bin/env python3
"""Counts the memory-map system calls (with strace, untimed), page faults, context switches and wall time of a
candidate under each allocator, interleaved, to explain a candidate whose wall time moves
while its cycles do not (threads waiting, for example on the mmap lock).

  syscalls.py NAME [--rounds 2] [--allocs glibc,mimalloc,asmalloc]

A measurement: run under scripts/measure (--both-l3 for candidates on both L3s) and
scripts/memlimit. Prints one line per run and appends them to ROOT/results/NAME/syscalls.jsonl.
"""
import argparse
import json
import random
import subprocess
import time

import screen

# Tracepoints are not readable here (tracefs is root's), so the system calls are counted by
# strace in a second, untimed run.
EVENTS = ["context-switches", "cpu-migrations", "page-faults", "task-clock"]


def strace_counts(path):
    counts = {}
    for line in path.read_text().splitlines():
        f = line.split()
        if len(f) >= 5 and f[-1].isidentifier() and f[3].isdigit():
            counts[f[-1]] = int(f[3])
    return counts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("name")
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--allocs", default="glibc,mimalloc,asmalloc")
    a = ap.parse_args()
    spec = screen.spec_of(a.name)
    if "server" in spec:
        raise SystemExit("servers are not supported here")
    scratch = screen.ROOT / "scratch" / a.name
    scratch.mkdir(parents=True, exist_ok=True)
    out = screen.outdir(a.name) / "syscalls.jsonl"
    for r in range(a.rounds):
        order = a.allocs.split(",")
        random.shuffle(order)
        for alloc in order:
            work = screen.setup(spec)
            lib = screen.ALLOCS[alloc]
            env = screen.target_env(spec, {"LD_PRELOAD": lib} if lib else {})
            stat = scratch / "sys.csv"
            wall = scratch / "wall"
            argv = ["perf", "stat", "-x,", "-o", str(stat), "-e", ",".join(EVENTS), "--", "taskset", "-c",
                    spec["cpus"], str(screen.WALLTIME), str(wall), "env"] + env + screen.argv_of(spec)
            screen.spin(spec["cpus"])
            subprocess.run(argv, cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=600)
            rec = {"name": a.name, "alloc": alloc, "round": r, "t": time.time(),
                   "wall_s": float(wall.read_text()), "counts": screen.parse_stat(stat)}
            work = screen.setup(spec)
            trace = scratch / "strace.txt"
            subprocess.run(["strace", "-f", "-qq", "--seccomp-bpf", "-c", "-o", str(trace), "-e",
                            "trace=mmap,munmap,madvise,mremap,mprotect,brk", "taskset", "-c", spec["cpus"], "env"]
                           + env + screen.argv_of(spec), cwd=work, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, timeout=600)
            rec["syscalls"] = strace_counts(trace)
            with open(out, "a") as f:
                f.write(json.dumps(rec) + "\n")
            c = rec["counts"]
            print(f"{a.name} {alloc:9} wall {rec['wall_s']:.3f}s " + " ".join(
                f"{k}={int(c.get(k) or 0)}" for k in EVENTS) + " " + " ".join(
                f"{k}={v}" for k, v in sorted(rec["syscalls"].items())), flush=True)


if __name__ == "__main__":
    main()
