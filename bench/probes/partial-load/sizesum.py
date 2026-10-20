#!/usr/bin/env python3
"""Allocations and frees per request by class, from run.py sizes runs (an asmalloc-sizes
build), beside each class's magazine capacity:

  sizesum.py DIR...
"""
import json
import sys
from pathlib import Path


def size(index):
    """The slot size of class `index` (crates/asmalloc/src/classes)."""
    if index <= 8:
        return index * 8
    k = index + 3
    return ((5 + (k & 3)) << ((k >> 2) - 2)) * 8


def capacity(sz):
    """Objects a full magazine of a class of `sz` bytes holds: 2^floor(log2(64 KiB / sz + 2))
    slots, within 4 and 1,024, less two."""
    fit = (64 << 10) // sz + 2
    log2 = min(max(fit.bit_length() - 1, 2), 10)
    return (1 << log2) - 2


for d in sys.argv[1:]:
    for p in sorted(Path(d).glob("*.sizes.json")):
        tag = p.name[: -len(".sizes.json")]
        n = json.loads((p.parent / f"{tag}.result.json").read_text())["groups"]["m"]["lat_us"]["n"]
        counts = {int(k): v for k, v in json.loads(p.read_text()).items()}
        print(f"\n{Path(d).name}/{tag} ({n} requests)")
        print("  class bytes  allocs/req  frees/req  magazine capacity")
        for index in sorted({i // 2 for i in counts}):
            a, f = counts.get(index * 2, 0) / n, counts.get(index * 2 + 1, 0) / n
            if a < 0.01 and f < 0.01:
                continue
            sz = size(index) if index < 63 else 0
            cap = capacity(sz) if sz and sz <= 32 << 10 else "-"
            print(f"  {sz if index < 63 else 'large':>11}  {a:10.2f}  {f:9.2f}  {cap:>17}")
