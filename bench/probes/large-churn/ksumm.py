#!/usr/bin/env python3
"""Medians of kern.py's records by job and arm: Mcycles (all, user), page faults, and the peak
(the cgroup's, MiB), with each arm's cycles against the first arm named.

  ksumm.py RUNS.jsonl [RUNS.jsonl...] [--base ARM]
"""
import argparse
import json
import statistics as st
from collections import defaultdict


def mib(peak):
    if not peak:
        return None
    v = peak.split()[0]
    unit = v[-1]
    n = float(v[:-1])
    return n * {"K": 1 / 1024, "M": 1, "G": 1024}.get(unit, 1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("runs", nargs="+")
    ap.add_argument("--base", default="main")
    a = ap.parse_args()
    rows = defaultdict(list)
    order = []
    for path in a.runs:
        for line in open(path):
            r = json.loads(line)
            if not r.get("result"):
                continue
            ph = r["result"]["phases"].values()
            key = (r["job"], r["arm"])
            if key not in rows:
                order.append(key)
            rows[key].append((sum(p["cycles"] for p in ph) / 1e6, sum(p.get("cycles_user", 0) for p in ph) / 1e6,
                              sum(p["page_faults"] for p in ph), mib(r.get("memlimit")) or r["result"].get("vm_hwm", 0) / 2**20))
    jobs = []
    for j, _ in order:
        if j not in jobs:
            jobs.append(j)
    for j in jobs:
        print(j)
        base = rows.get((j, a.base))
        bc = st.median(x[0] for x in base) if base else None
        for (jj, arm) in order:
            if jj != j:
                continue
            v = rows[(jj, arm)]
            c = st.median(x[0] for x in v)
            rel = f"{100 * (c / bc - 1):+6.1f}%" if bc else ""
            print(f"  {arm:8} n={len(v)} Mcyc {c:8.1f} ({min(x[0] for x in v):.0f}-{max(x[0] for x in v):.0f}) {rel} "
                  f"user {st.median(x[1] for x in v):8.1f} faults {st.median(x[2] for x in v):9.0f} peak {st.median(x[3] for x in v):7.1f}")


if __name__ == "__main__":
    main()
