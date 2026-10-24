#!/usr/bin/env python3
"""ticks.py PERF.DATA [--match TEXT,...]: where in each millisecond of CLOCK_MONOTONIC the
server CPUs spend their time (perf recorded with -k mono and -g, trace.sh).

Reads `perf script` and bins every sample by its time's offset in the millisecond (25 us bins):
the samples whose call chain holds one of the --match texts (default: the timer interrupt), and
of those the leaf functions that take most of them. Also prints the top call-chain frames of
the matching samples.
"""
import argparse
import subprocess
from collections import Counter


def samples(path):
    p = subprocess.Popen(["perf", "script", "-i", path, "-F", "comm,tid,cpu,time,ip,sym"],
                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    cur = None
    for line in p.stdout:
        if not line.strip():
            if cur:
                yield cur
            cur = None
            continue
        if not line.startswith((" ", "\t")) or cur is None:
            parts = line.split()
            # comm may contain spaces: find the [cpu] field
            i = next(k for k, x in enumerate(parts) if x.startswith("[") and x.endswith("]"))
            t = float(parts[i + 1].rstrip(":"))
            cur = {"comm": " ".join(parts[:i - 1]), "tid": parts[i - 1], "cpu": int(parts[i][1:-1]),
                   "t": t, "chain": []}
        else:
            f = line.split(None, 1)
            if len(f) == 2:
                cur["chain"].append(f[1].strip().split(" (")[0])
    if cur:
        yield cur


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("data")
    ap.add_argument("--match", default="sysvec_apic_timer_interrupt")
    a = ap.parse_args()
    keys = a.match.split(",")
    allb, hit = Counter(), Counter()
    leaf, frames = Counter(), Counter()
    n = 0
    for s in samples(a.data):
        n += 1
        b = int((s["t"] * 1e9) % 1_000_000 // 25_000)
        allb[b] += 1
        if any(k in fr for fr in s["chain"] for k in keys):
            hit[b] += 1
            leaf[s["chain"][0] if s["chain"] else "?"] += 1
            for fr in set(s["chain"]):
                frames[fr] += 1
    print(f"{n} samples, {sum(hit.values())} match {keys}")
    print("offset in the ms (us): all samples, matching")
    for b in range(40):
        print(f"  {b * 25:4d} {allb[b]:7d} {hit[b]:6d}")
    print("matching leaves: " + ", ".join(f"{k} {v}" for k, v in leaf.most_common(15)))
    print("matching frames: " + ", ".join(f"{k} {v}" for k, v in frames.most_common(30)))


if __name__ == "__main__":
    main()
