#!/usr/bin/env python3
"""cpieces.py RUN... [--slow US]: per piece of each run's load plan (s30, s70, t00 to t19), the
server CPUs' idle-state entries a second (POLL, C1, C2, C3; from cstate.py's samples) beside
the piece's p99.9 and share of requests slower than --slow; then the correlation over pieces
of C2 entries with the slow share. RUN is a trace.sh prefix.
"""
import argparse
import bisect
import json
import struct
from pathlib import Path

SERVER = ("0", "1", "2", "3")


def q(xs, p):
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else 0.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("runs", nargs="+")
    ap.add_argument("--slow", type=float, default=42.0)
    a = ap.parse_args()
    pts = []
    for run in a.runs:
        rec = next(json.loads(l) for l in open(run + ".jsonl") if '"type":"run"' in l.replace(" ", ""))
        base = rec["served"]["generator"]["start_mono_ns"]
        segs = rec["served"]["generator"]["segments"]
        raw = next(Path(run + ".raw").glob("*.raw"))
        lat = {}
        for d, l, s, f in struct.iter_unpack("<QQHH", raw.read_bytes()):
            if not f & 4:
                lat.setdefault(s, []).append(l / 1e3)
        samples = [json.loads(x) for x in open(run + ".cstate.jsonl")]
        ts = [x["t"] for x in samples]
        print(f"\n{run.split('/')[-1]} ({rec['allocator']}): piece, p99.9 us, slow %, server CPUs' entries/s POLL C1 C2 C3, C2 ms/s")
        for i, s in enumerate(segs):
            if not (s["name"] in ("s30", "s70") or s["name"].startswith("t")):
                continue
            t0, t1 = base + s["begin_ms"] * 1e6, base + s["end_ms"] * 1e6
            i0, i1 = bisect.bisect_left(ts, t0), bisect.bisect_right(ts, t1) - 1
            if i1 <= i0:
                continue
            dt = (ts[i1] - ts[i0]) / 1e9
            ent = [sum(samples[i1]["cpu"][c][k][0] - samples[i0]["cpu"][c][k][0] for c in SERVER) / dt for k in range(4)]
            c2ms = sum(samples[i1]["cpu"][c][2][1] - samples[i0]["cpu"][c][2][1] for c in SERVER) / dt / 1e3
            xs = sorted(lat.get(i, []))
            slow = 100 * sum(1 for x in xs if x > a.slow) / max(1, len(xs))
            pts.append((ent[2], slow))
            print(f"  {s['name']:4} {q(xs, .999):6.1f} {slow:6.3f}   {ent[0]:7.0f} {ent[1]:7.0f} {ent[2]:7.0f} {ent[3]:5.0f}   {c2ms:6.2f}")
    if len(pts) > 2:
        n = len(pts)
        mx, my = sum(p[0] for p in pts) / n, sum(p[1] for p in pts) / n
        sxy = sum((x - mx) * (y - my) for x, y in pts)
        sxx = sum((x - mx) ** 2 for x, _ in pts)
        syy = sum((y - my) ** 2 for _, y in pts)
        print(f"\ncorrelation of C2 entries/s with the slow share over {n} pieces: {sxy / (sxx * syy) ** 0.5:.2f}")


if __name__ == "__main__":
    main()
