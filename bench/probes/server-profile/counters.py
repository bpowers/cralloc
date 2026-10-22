#!/usr/bin/env python3
"""The runner's per-request counters at 70% (`s70`) for each run directory given: cycles,
instructions, faults, and the raw events record.py passed (dTLB misses, page walks, L1 fills,
user mode only).

  counters.py DIR...
"""
import json
import sys
from pathlib import Path

for d in sys.argv[1:]:
    p = Path(d) / "run.jsonl"
    if not p.exists():
        continue
    for line in p.open():
        r = json.loads(line)
        g = r.get("served", {}).get("groups", {}).get("s70")
        if not g:
            continue
        s = g["server"]
        keys = ["cycles_per_req", "instructions_per_req", "faults_per_req", "dtlb_per_req", "walk_per_req",
                "fills_per_req"]
        print(f"{d:40} " + "  ".join(f"{k.replace('_per_req', '')} {s[k]:,.1f}" for k in keys if s.get(k) is not None)
              + f"  p99.9 {g['lat_us']['p999']} us")
