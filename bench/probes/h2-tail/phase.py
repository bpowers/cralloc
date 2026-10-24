#!/usr/bin/env python3
"""phase.py RESULTS.jsonl RAWDIR [--slow US] [--from S]: the phase of each run's slow requests
within a millisecond of CLOCK_MONOTONIC (the generator's start_mono_ns plus the due time), so
that runs can be compared: a phase that stays put across runs is tied to the machine's clock
(the timer tick), one that moves is tied to the run's own start (the arrival schedule).

Prints per run the circular mean phase in us of the slow requests' due and end times, and the
concentration (0 uniform, 1 a single phase).
"""
import argparse
import cmath
import json
import math
import struct
from pathlib import Path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("rawdir")
    ap.add_argument("--slow", type=float, default=42.0)
    ap.add_argument("--from", dest="t0", type=float, default=17.0)
    a = ap.parse_args()
    runs = [json.loads(l) for l in open(a.results) if l.strip()]
    runs = sorted((r for r in runs if r.get("type") == "run"), key=lambda r: r.get("seq", 0))
    raws = sorted(Path(a.rawdir).glob("*.raw"), key=lambda p: p.stat().st_mtime)
    for r, f in zip(runs, raws):
        base = r["served"]["generator"]["start_mono_ns"]
        due, end = [], []
        for d, l, s, fl in struct.iter_unpack("<QQHH", f.read_bytes()):
            if d >= a.t0 * 1e9 and not fl & 4 and l > a.slow * 1e3:
                due.append((base + d) % 1_000_000)
                end.append((base + d + l) % 1_000_000)

        def circ(xs):
            z = sum(cmath.exp(2j * math.pi * x / 1e6) for x in xs) / max(1, len(xs))
            return (cmath.phase(z) % (2 * math.pi)) / (2 * math.pi) * 1000, abs(z)

        (pd, cd), (pe, ce) = circ(due), circ(end)
        print(f"{r['allocator']:18} start%1ms {base % 1_000_000 / 1e3:6.1f}us  slow {len(due):5d}  "
              f"due phase {pd:6.1f}us ({cd:.2f})  end phase {pe:6.1f}us ({ce:.2f})")


if __name__ == "__main__":
    main()
