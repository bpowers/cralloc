#!/usr/bin/env python3
"""Memory of large-heap server runs by arm, from benchrun's records and the default-thread
watcher's (bench/probes/default-thread/watch.py) beside them:

  htsum.py DIR TAG... [--threads]

Per run: the anonymous resident memory from the runner's rollups (the process's heap) and the
run cgroup's anonymous memory from the watcher (`memory.stat` anon, which counts what a
process has in huge pages the same way), each as mean and peak over the plan's 70% segments
(first `s70` to the first `off`) and the last value of the run (a minute after close in
`servers-thread`); AnonHugePages at the peak; page faults, cycles and tails at 70%. With
--threads, which search runtime the run settled on (threadcpu.py's TAG.threads.jsonl, in run
order). Then medians by arm. MiB.
"""
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

M = 2**20


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else float("nan")


def seg_bounds(run):
    segs = run["served"]["generator"]["segments"]
    s70 = [s for s in segs if s["name"] == "s70"]
    off = [s for s in segs if s["name"] == "off"]
    return s70[0]["begin_ms"], (off[0]["begin_ms"] if off else s70[-1]["end_ms"])


def main():
    d = Path(sys.argv[1])
    tags = [a for a in sys.argv[2:] if not a.startswith("--")]
    rows = []
    for tag in tags:
        runs = [json.loads(l) for l in open(d / f"{tag}.jsonl") if '"type":"run"' in l]
        watch = [json.loads(l) for l in open(d / f"{tag}.watch.jsonl")]
        procs = [w for w in watch if w["type"] == "proc"]
        gens = sorted(w["start"] for w in watch if w["type"] == "gen")
        thr = []
        tf = d / f"{tag}.threads.jsonl"
        if "--threads" in sys.argv and tf.exists():
            thr = [json.loads(l) for l in open(tf)]
        for i, r in enumerate(runs):
            if not r.get("valid") or not r.get("served"):
                rows.append((r["allocator"], None))
                continue
            lo, hi = seg_bounds(r)
            cand = [p for p in procs if abs(p["start"] - r["started"]) < 5]
            samples = {p["pid"]: [w for w in watch if w["type"] == "s" and w.get("pid") == p["pid"]] for p in cand}
            # The server: the process with the largest resident set (a driver may run it in a
            # child of the same binary). The cgroup's figures are the same from either.
            pid = max(samples, key=lambda p: max((s["rss"] for s in samples[p]), default=0)) if samples else None
            first = min((p["start"] for p in cand), default=r["started"])
            gen = next((g for g in gens if g >= first), None)
            cg = [s["cg_anon"] for s in samples.get(pid, []) if gen and lo <= (s["t"] - gen) * 1e3 <= hi]
            # The last sample while the server lived: its resident set still above a few MiB.
            alive = [s for s in samples.get(pid, []) if s["rss"] > 64 * M]
            cg_end = alive[-1]["cg_anon"] if alive else None
            curve = [p for p in r["served"]["curve_kib"] if lo <= p[0] <= hi]
            anon = [p[2] * 1024 for p in curve if p[2]]
            g = r["served"]["groups"]["s70"]
            mode = ""
            if i < len(thr):
                c = thr[i]["cpu_s"]
                mode = "io" if c.get("search-io-#", 0) > c.get("search-cpu-#", 0) else "cpu"
            rows.append((r["allocator"], {
                "anon_mean": statistics.mean(anon) / M if anon else None,
                "anon_peak": max(anon) / M if anon else None,
                "anon_end": r["served"]["curve_kib"][-1][2] * 1024 / M,
                "cg_mean": statistics.mean(cg) / M if cg else None,
                "cg_peak": max(cg) / M if cg else None,
                "cg_end": cg_end / M if cg_end else None,
                "thp": (r["memory"].get("anon_huge_peak") or 0) / M,
                "faults": g["server"]["faults_per_req"],
                "cyc": g["server"]["cycles_per_req"] / 1e6,
                "p99": g["lat_us"]["p99"] / 1e3,
                "p999": g["lat_us"]["p999"] / 1e3,
                "mode": mode,
            }))
    by = defaultdict(list)
    for a, v in rows:
        if v:
            by[a].append(v)
            print(f"  {a:12s} {v['mode']:3s} anon {v['anon_mean']:6.0f} {v['anon_peak']:6.0f} {v['anon_end']:6.0f} | cgroup anon "
                  f"{v['cg_mean'] or 0:6.0f} {v['cg_peak'] or 0:6.0f} {v['cg_end'] or 0:6.0f} | thp {v['thp']:5.0f} | "
                  f"flt/req {v['faults']:5.1f} | {v['cyc']:6.2f} Mcyc | p99/p99.9 {v['p99']:5.0f} {v['p999']:5.0f} ms")
        else:
            print(f"  {a:12s} invalid")
    print("\nmedians: anon mean / peak / end | cgroup anon mean / peak / end | AnonHugePages peak | faults/req | p99 / p99.9 ms")
    for a, vs in by.items():
        m = lambda k: med([v[k] for v in vs])
        print(f"  {a:12s} n={len(vs)}  {m('anon_mean'):6.0f} {m('anon_peak'):6.0f} {m('anon_end'):6.0f} | {m('cg_mean'):6.0f} "
              f"{m('cg_peak'):6.0f} {m('cg_end'):6.0f} | {m('thp'):5.0f} | {m('faults'):5.1f} | {m('p99'):5.0f} {m('p999'):5.0f}")


if __name__ == "__main__":
    main()
