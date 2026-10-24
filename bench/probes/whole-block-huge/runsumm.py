#!/usr/bin/env python3
"""benchrun's records of the patched runner (sessions/run.sh), by job and allocator, for
docs/research/whole-block-huge.md. One line a run, then medians.

  runsumm.py [--group s70] RUNS.jsonl...

A server run: its group's cycles, faults, dTLB misses and 4 KiB and 2 MiB walks per request
(the walks from the runner's process counters, user mode), p99, the anonymous memory's mean
and peak over the group's segments (MiB, from the curve), the peak and mean AnonHugePages,
and the machine's THP faults and fallbacks and compaction stalls over the run. A batch run
(DataFusion): the primary metric, faults, the walks over the process, the cgroup peak.
"""
import json
import statistics as st
import sys
from collections import defaultdict


def server_line(d, group):
    s = d["served"]
    g = s["groups"][group]
    sv, lat = g["server"], g["lat_us"]
    n = lat["n"] or 1
    ex = sv.get("extra", {})
    segs = [x for x in s["generator"]["segments"] if x["name"] == group]
    lo, hi = segs[0]["begin_ms"], segs[-1]["end_ms"]
    anon = [p[2] / 1024 for p in s["curve_kib"] if lo <= p[0] <= hi] or [0]
    m, vm = d["memory"], d.get("vmstat", {})
    return {
        "Mcyc/req": sv["cycles_per_req"] / 1e6, "flt/req": sv["faults_per_req"],
        "miss/req": ex.get("dtlb_miss", 0) / n, "w4k/req": ex.get("walk_4k", 0) / n, "w2m/req": ex.get("walk_2m", 0) / n,
        "p99ms": lat["p99"] / 1e3, "anon_mean": sum(anon) / len(anon), "anon_peak": max(anon),
        "huge_peak": m.get("anon_huge_peak", 0) / 2**20, "huge_mean": m.get("anon_huge_mean", 0) / 2**20,
        "thpfa": vm.get("thp_fault_alloc", 0), "thpfb": vm.get("thp_fault_fallback", 0), "stall": vm.get("compact_stall", 0),
    }


def batch_line(d):
    pr = dict(d["process"])
    # Without the runner's raw events, the program's own phases have them (BENCH_EVENTS).
    for ph in ((d.get("workload") or {}).get("phases") or {}).values():
        for k in ("dtlb_miss", "walk_4k", "walk_2m", "walk"):
            if k in ph and k not in d["process"]:
                pr[k] = pr.get(k, 0) + ph[k]
    return {
        "Gcyc": d["primary"]["value"], "faults_k": d["rusage"]["minflt"] / 1e3,
        "miss_M": pr.get("dtlb_miss", 0) / 1e6, "w4k_M": pr.get("walk_4k", 0) / 1e6, "w2m_M": pr.get("walk_2m", 0) / 1e6,
        "walk_M": pr.get("walk", 0) / 1e6,
        "peak": d["cgroup"]["peak"] / 2**20, "huge_peak": d["memory"].get("anon_huge_peak", 0) / 2**20,
        "thpfa": d.get("vmstat", {}).get("thp_fault_alloc", 0), "stall": d.get("vmstat", {}).get("compact_stall", 0),
    }


def main():
    args = sys.argv[1:]
    group = "s70"
    if args[0] == "--group":
        group, args = args[1], args[2:]
    rows = defaultdict(list)
    for path in args:
        for line in open(path):
            d = json.loads(line)
            if d.get("type") != "run":
                continue
            if not d.get("valid"):
                print(f"invalid: {d['job']} {d['allocator']} {d.get('problems')}")
            v = server_line(d, group) if d.get("served") else batch_line(d)
            rows[(d["job"], d["allocator"])].append(v)
            print(f"{d['job'][:26]:26} {d['allocator']:9} " + " ".join(f"{k} {x:.3g}" for k, x in v.items()))
    print("medians")
    for (job, alloc), vs in sorted(rows.items()):
        keys = vs[0].keys()
        print(f"{job[:26]:26} {alloc:9} n={len(vs)} " + " ".join(f"{k} {st.median(v[k] for v in vs):.4g}" for k in keys))


if __name__ == "__main__":
    main()
