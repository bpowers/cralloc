#!/usr/bin/env python3
"""Resident memory in free large slots over a drift run (docs/research/large-slack.md), from the
breakdowns asmalloc's memstat build writes (`account_every`): for each breakdown, the sum over
the large classes (slots over 32 KiB) of the resident bytes of their free slots in the bitmaps.

    python3 -I free_resident.py RUN.txt...

Prints, per run, the mean and the largest over the breakdowns taken while the workload ran
(its phases), in MiB, and the heap over live at the quiet minute's first and last samples;
with a breakdown in the quiet minute, where its last one's resident heap sits.
"""
import json
import re
import sys
from pathlib import Path

KV = re.compile(r"(\w+)=(\S+)")
MiB = 2**20


def first(v):
    return int(v.split("/")[0])


def breakdowns(memstat):
    out, cur = [], None
    for line in Path(memstat).read_text().splitlines():
        if line.startswith("acct "):
            kv = dict(KV.findall(line))
            cur = {"t": int(kv["t_ms"]), "large": 0, "small": 0, "acct": kv}
            out.append(cur)
        elif line.startswith("acct-class ") and cur is not None:
            kv = dict(KV.findall(line))
            size = int(kv["size"])
            cur["large" if size > 32768 else "small"] += first(kv["free"])
    return out


for run in sys.argv[1:]:
    rows = [json.loads(l[6:]) for l in Path(run).read_text().splitlines() if l.startswith("DRIFT ")]
    load_end = max((r["t_ms"] for r in rows if r["kind"] == "load"), default=0)
    bd = breakdowns(run + ".memstat")
    # A breakdown is taken just after the sample that asks for it.
    load = [b["large"] for b in bd if b["t"] <= load_end + 1000]
    quiet = [b for b in bd if b["t"] > load_end + 1000]
    q = quiet[-1] if quiet else None
    qr = [r["ratio"] for r in rows if r["kind"] == "quiet"]
    name = Path(run).name
    if not load:
        print(f"{name}: no breakdowns")
        continue
    line = f"{name}: free large resident over the phases mean {sum(load) / len(load) / MiB:.0f} MiB, max {max(load) / MiB:.0f} MiB ({len(load)} breakdowns)"
    if qr:
        line += f"; quiet ratio {qr[0]:.3f} -> {qr[-1]:.3f}"
    if q:
        a = q["acct"]
        parts = ", ".join(f"{k} {int(a[k]) / MiB:.0f}" for k in ("live", "loaded", "spare", "depot", "free", "above", "meta", "mags") if k in a)
        line += f"; quiet end: free large {q['large'] / MiB:.0f} MiB, free small {q['small'] / MiB:.0f} MiB, heap {int(a['heap']) / MiB:.0f} MiB ({parts})"
    print(line)
