#!/usr/bin/env python3
"""monophase.py RUN [--slow US] [--bin US] [--from S]: requests by the offset of their due time
within a millisecond of CLOCK_MONOTONIC, the clock the timer tick keeps: per bin, the requests,
their mean latency and the share slower than --slow. RUN is a trace.sh or session.sh prefix
with RUN.jsonl and RUN.raw/ holding one run.
"""
import argparse
import json
import struct
from collections import Counter
from pathlib import Path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("run")
    ap.add_argument("--slow", type=float, default=42.0)
    ap.add_argument("--bin", type=float, default=25.0)
    ap.add_argument("--from", dest="t0", type=float, default=20.0)
    a = ap.parse_args()
    rec = next(json.loads(l) for l in open(a.run + ".jsonl") if '"type":"run"' in l.replace(" ", ""))
    base = rec["served"]["generator"]["start_mono_ns"]
    raw = next(Path(a.run + ".raw").glob("*.raw"))
    nb = int(1000 / a.bin)
    n, lat, slow = Counter(), Counter(), Counter()
    for d, l, s, f in struct.iter_unpack("<QQHH", raw.read_bytes()):
        if f & 4 or d < a.t0 * 1e9:
            continue
        b = int(((base + d) % 1_000_000) / (a.bin * 1e3))
        n[b] += 1
        lat[b] += l
        slow[b] += l > a.slow * 1e3
    print(f"{rec['allocator']}: offset us, requests, mean latency us, slow %")
    for b in range(nb):
        bar = "#" * int(200 * slow[b] / max(1, n[b]))
        print(f"  {b * a.bin:5.0f} {n[b]:7d} {lat[b] / max(1, n[b]) / 1e3:6.2f} {100 * slow[b] / max(1, n[b]):6.3f} {bar}")


if __name__ == "__main__":
    main()
