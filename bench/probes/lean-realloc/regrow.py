#!/usr/bin/env python3
"""Instructions and cycles per `realloc` of a small block for several builds of the workload
binary, interleaved: `workload regrow` (bench/workloads/src/single.rs) for each pattern, each
build in turn, ROUNDS times, then each build's median. Run it from anywhere under
`scripts/measure` (one CPU of the first L3, so no `--both-l3`); each run goes through
`scripts/memlimit`.

usage: regrow.py ROUNDS NAME=BINARY...
"""
import json
import os
import statistics
import subprocess
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../.."))
PATTERNS = [
    ("vec 128->256", ["pattern=vec", "from=128"]),
    ("vec 32->64", ["pattern=vec", "from=32"]),
    ("vec 1Ki->2Ki", ["pattern=vec", "from=1024"]),
    ("string 8->1Ki", ["pattern=string", "to=1024"]),
]


def run(binary, args, cpu):
    out = subprocess.run(
        [f"{ROOT}/scripts/memlimit", "-m", "1G", "-t", "5m", "--", binary, "regrow", *args],
        env={**os.environ, "BENCH_CPUS": cpu, "MEMLIMIT_QUIET": "1"},
        capture_output=True, text=True, check=True).stdout
    line = next(l for l in out.splitlines() if l.startswith("BENCH_RESULT "))
    metrics = json.loads(line.split(" ", 1)[1])["metrics"]
    return metrics["instructions/realloc"], metrics["cycles/realloc"]


def main():
    rounds = int(sys.argv[1])
    builds = [a.split("=", 1) for a in sys.argv[2:]]
    cpu = os.environ.get("BENCH_CPUS", "2")
    got = {}
    for _ in range(rounds):
        for label, args in PATTERNS:
            for name, binary in builds:
                got.setdefault((label, name), []).append(run(binary, args, cpu))
    head = "".join(f"{n + ' instr':>18}{n + ' cycles':>18}" for n, _ in builds)
    print(f"{'pattern':16}{head}")
    for label, _ in PATTERNS:
        row = f"{label:16}"
        for name, _ in builds:
            v = got[(label, name)]
            row += f"{statistics.median(i for i, _ in v):>18.1f}"
            row += f"{statistics.median(c for _, c in v):>18.1f}"
        print(row)


main()
