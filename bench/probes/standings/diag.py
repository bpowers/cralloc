"""First diagnosis of a gap: diag.py JOBTEXT ALLOC,ALLOC,... RESULTS.jsonl [--rounds N]

Runs the job's recorded command line for each allocator, interleaved, under `perf stat` (user
and kernel cycles, instructions, faults, context switches, dTLB walks, L2 misses) pinned as the
suite pins it, then once under `strace -c -f` for the system calls. Prints medians. Run it under
scripts/measure (--both-l3 when the job uses CPUs 8 and up); each run is memory capped."""
import argparse
import json
import os
import statistics
import subprocess
import tempfile
from collections import defaultdict

# The checkout this script is in, for scripts/memlimit.
ME = str(__import__("pathlib").Path(__file__).resolve().parents[3])
EVENTS = ["cycles:u", "cycles:k", "instructions:u", "page-faults", "context-switches",
          "dTLB-load-misses", "L1-dcache-load-misses"]
TUNABLES = {"tcmalloc-percpu": "glibc.pthread.rseq=0"}


def runs_of(path, want, allocs):
    seen = {}
    for line in open(path):
        r = json.loads(line)
        if r.get("type") == "run" and r["job"] == want and r["allocator"] in allocs:
            seen.setdefault(r["allocator"], r)
    return seen


def env_for(alloc, cpus):
    env = dict(os.environ, BENCH_CPUS=cpus, MEMLIMIT_QUIET="1")
    if alloc in TUNABLES:
        env["GLIBC_TUNABLES"] = TUNABLES[alloc]
    return env


def perf(rec):
    with tempfile.NamedTemporaryFile("r", suffix=".csv") as out:
        cmd = [f"{ME}/scripts/memlimit", "-m", "6G", "-t", "10m", "--", "perf", "stat", "-x,",
               "-o", out.name, "-e", ",".join(EVENTS), "taskset", "-c", rec["cpus"]] + rec["argv"]
        subprocess.run(cmd, env=env_for(rec["allocator"], rec["cpus"]), stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, check=False)
        vals = {}
        for line in out.read().splitlines():
            parts = line.split(",")
            if len(parts) > 2 and parts[0] and parts[0][0].isdigit():
                vals[parts[2]] = float(parts[0])
        return vals


def strace(rec):
    with tempfile.NamedTemporaryFile("r", suffix=".txt") as out:
        cmd = [f"{ME}/scripts/memlimit", "-m", "6G", "-t", "10m", "--", "strace", "-c", "-f",
               "-o", out.name, "taskset", "-c", rec["cpus"]] + rec["argv"]
        subprocess.run(cmd, env=env_for(rec["allocator"], rec["cpus"]), stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, check=False)
        calls = {}
        for line in out.read().splitlines():
            parts = line.split()
            if len(parts) >= 5 and parts[0][0].isdigit() and parts[-1] not in ("total",):
                calls[parts[-1]] = int(parts[3])
        return calls


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("job")
    ap.add_argument("allocs")
    ap.add_argument("results")
    ap.add_argument("--rounds", type=int, default=3)
    a = ap.parse_args()
    # A job name's spaces may be given as '%', for callers that split on spaces.
    a.job = a.job.replace("%", " ")
    allocs = a.allocs.split(",")
    recs = runs_of(a.results, a.job, allocs)
    stats = defaultdict(lambda: defaultdict(list))
    for _ in range(a.rounds):
        for alloc in allocs:
            for k, v in perf(recs[alloc]).items():
                stats[alloc][k].append(v)
    print(f"== {a.job}: medians of {a.rounds} runs")
    print(f"{'event':24}" + "".join(f"{x:>18}" for x in allocs))
    for e in EVENTS:
        print(f"{e:24}" + "".join(f"{statistics.median(stats[x][e]) if stats[x][e] else float('nan'):18.4g}"
                                  for x in allocs))
    sc = {x: strace(recs[x]) for x in allocs}
    names = sorted(set().union(*sc.values()), key=lambda n: -max(sc[x].get(n, 0) for x in allocs))[:12]
    print(f"{'system call':24}" + "".join(f"{x:>18}" for x in allocs))
    for n in names:
        print(f"{n:24}" + "".join(f"{sc[x].get(n, 0):18}" for x in allocs))


main()
