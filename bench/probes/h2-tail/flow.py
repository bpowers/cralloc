#!/usr/bin/env python3
"""flow.py RAW [--slow US] [--from S]: the server's HTTP/2 flow control seen from the load
generator (loadgen's record flags 8 and 16, and RAW.win): how often a request's body waited
for a window, whether the slow requests are those, and the windows the server granted.
"""
import argparse
import struct
from collections import Counter, defaultdict
from pathlib import Path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("raw")
    ap.add_argument("--slow", type=float, default=42.0)
    ap.add_argument("--from", dest="t0", type=float, default=17.0)
    a = ap.parse_args()
    recs = [(d, l, f) for d, l, s, f in struct.iter_unpack("<QQHH", Path(a.raw).read_bytes())
            if d >= a.t0 * 1e9 and not f & 4]
    slow = [r for r in recs if r[1] > a.slow * 1e3]
    for name, bit in (("connection window", 8), ("stream window", 16)):
        n = sum(1 for r in recs if r[2] & bit)
        ns = sum(1 for r in slow if r[2] & bit)
        print(f"{name}: {n} of {len(recs)} requests waited ({100 * n / max(1, len(recs)):.3f}%), "
              f"{ns} of {len(slow)} slow ones ({100 * ns / max(1, len(slow)):.1f}%)")
    w = Path(str(a.raw) + ".win")
    if not w.exists():
        return
    ev = list(struct.iter_unpack("<QHHI", w.read_bytes()))
    settings = defaultdict(list)
    incs = defaultdict(Counter)
    for t, c, k, v in ev:
        if k == 1:
            settings[c].append((t / 1e9, v))
        else:
            incs[c][v // 1024] += 1
    for c in sorted(set(settings) | set(incs)):
        s = ", ".join(f"{v}@{t:.2f}s" for t, v in settings[c])
        top = ", ".join(f"{k}KiB x{n}" for k, n in incs[c].most_common(3))
        print(f"  conn {c}: initial stream windows {s or '-'}; connection increments {top}")


if __name__ == "__main__":
    main()
