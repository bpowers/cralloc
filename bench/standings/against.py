#!/usr/bin/env python3
"""One allocator's server jobs against an earlier standings: against.py --old FILE... --new FILE...

For each server job in both sets (allocators `NAME@padNN` folded into NAME), the allocator's
(default cralloc) cycles a request at s70 as the mean over pads of the per-pad medians,
page faults a request at s70 (median over runs), p99.9 (median and range over runs), the
resident peak under load and at the end of the plan (medians), old against new, and flags: cycles more
than 2% up, faults a tenth or more up (and at least 0.05 a request), a p99.9 median outside
the old runs' range.
"""
import argparse
import gzip
import json
import statistics
from collections import defaultdict

MiB = 2**20


def runs(files, want):
    by = defaultdict(list)
    for f in files:
        op = gzip.open if f.endswith(".gz") else open
        with op(f, "rt") as fh:
            for line in fh:
                if '"type":"run"' not in line and '"type": "run"' not in line:
                    continue
                r = json.loads(line)
                name, _, pad = r["allocator"].partition("@")
                if name != want or r.get("valid") is not True:
                    continue
                r["_pad"] = pad or "pad00"
                by[r["job"]].append(r)
    return by


def at_end(served):
    """The resident MiB at the end of the plan. The curve goes on past it into the server's
    shutdown, whose frees would count as memory it gave back while idle."""
    end = next((m[0] for m in served.get("marks_ms") or [] if m[1] == "end"), float("inf"))
    c = [p for p in served.get("curve_kib") or [] if p[0] <= end]
    return c[-1][1] / 1024 if c else 0


def summary(rs):
    per = defaultdict(list)
    for r in rs:
        per[r["_pad"]].append(r["primary"]["value"])
    cyc = statistics.mean(statistics.median(v) for v in per.values())
    out = {"cyc": cyc / 1e3, "n": len(rs)}
    g = [r["served"]["groups"].get("s70") for r in rs if r.get("served")]
    g = [x for x in g if x]
    if g:
        p = [x["lat_us"]["p999"] for x in g]
        out |= {
            "faults": statistics.median(x["server"]["faults_per_req"] for x in g),
            "p999": statistics.median(p), "p999_lo": min(p), "p999_hi": max(p),
            "peak": statistics.median(r["served"].get("rss_max_loaded", 0) / MiB for r in rs),
            "end": statistics.median(at_end(r["served"]) for r in rs),
        }
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--old", nargs="+", required=True)
    ap.add_argument("--new", nargs="+", required=True)
    ap.add_argument("--alloc", default="cralloc")
    a = ap.parse_args()
    old, new = runs(a.old, a.alloc), runs(a.new, a.alloc)
    for job in sorted(set(old) & set(new)):
        o, n = summary(old[job]), summary(new[job])
        flags = []
        d = 100 * (n["cyc"] / o["cyc"] - 1)
        if d > 2:
            flags.append("SLOWER")
        line = f"{job:24} cycles k {o['cyc']:8.2f} -> {n['cyc']:8.2f} ({d:+.1f}%)"
        if "faults" in o and "faults" in n:
            if n["faults"] >= 1.1 * o["faults"] and n["faults"] - o["faults"] >= 0.05:
                flags.append("FAULTS")
            if not (o["p999_lo"] <= n["p999"] <= o["p999_hi"]):
                flags.append("P999-OUTSIDE")
            line += (f" | faults/req {o["faults"]:.4f} -> {n["faults"]:.4f}"
                     f" | p99.9 {o['p999']:.1f} ({o['p999_lo']:.1f}-{o['p999_hi']:.1f}) -> {n['p999']:.1f} ({n['p999_lo']:.1f}-{n['p999_hi']:.1f})"
                     f" | peak {o['peak']:.0f} -> {n['peak']:.0f} end {o['end']:.0f} -> {n['end']:.0f}")
        print(line + ("  " + " ".join(flags) if flags else ""))


if __name__ == "__main__":
    main()
