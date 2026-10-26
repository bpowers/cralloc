#!/usr/bin/env python3
"""Summarizes kern.py's records: per job and arm, each round's Mcycles (all phases), the
median, faults and peak RSS (docs/research/large-slack.md).

    python3 -I kern_summary.py OUT.jsonl
"""
import json
import statistics
import sys
from collections import defaultdict

by = defaultdict(list)
for line in open(sys.argv[1]):
    r = json.loads(line)
    ph = (r.get("result") or {}).get("phases", {})
    cyc = sum(v.get("cycles", 0) for v in ph.values()) / 1e6
    pf = sum(v.get("page_faults", 0) for v in ph.values())
    hwm = (r.get("result") or {}).get("vm_hwm", 0) / 2**20
    by[(r["job"], r["arm"])].append((cyc, pf, hwm))
for (job, arm), rs in sorted(by.items()):
    cs = ", ".join(f"{c:.0f}" for c, _, _ in rs)
    print(f"{job[:32]:32} {arm:5} Mcyc {cs:24} median {statistics.median(c for c, _, _ in rs):6.0f}"
          f"  faults {statistics.median(p for _, p, _ in rs):7.0f}  hwm {max(h for _, _, h in rs):6.1f} MiB")
