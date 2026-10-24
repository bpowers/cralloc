#!/usr/bin/env python3
"""runsum.py RUN... [--slow US]: one line per trace.sh run over its 70%-rate pieces (s70 and
t00 to t19 together): server cycles a request at s70, the pooled p99 and p99.9, the share
slower than --slow, and the server CPUs' idle-state entries over those pieces, a second and a
request (POLL, C1, C2+C3), from cstate.py's samples.
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
    print(f"{'run':34} {'kcyc':>6} {'rate':>6} {'p99':>5} {'p99.9':>6} {'slow%':>6}  {'POLL/s':>7} {'C1/s':>7} {'C2+C3/s':>7}  {'C1/req':>6} {'deep/kreq':>9}")
    for run in a.runs:
        rec = next(json.loads(l) for l in open(run + ".jsonl") if '"type":"run"' in l.replace(" ", ""))
        base = rec["served"]["generator"]["start_mono_ns"]
        segs = rec["served"]["generator"]["segments"]
        idx = [i for i, s in enumerate(segs) if s["name"] == "s70" or s["name"].startswith("t")]
        t0 = base + segs[idx[0]]["begin_ms"] * 1e6
        t1 = base + segs[idx[-1]]["end_ms"] * 1e6
        want = set(idx)
        lat = sorted(l / 1e3 for d, l, s, f in struct.iter_unpack("<QQHH", next(Path(run + ".raw").glob("*.raw")).read_bytes())
                     if s in want and not f & 4)
        samples = [json.loads(x) for x in open(run + ".cstate.jsonl")]
        ts = [x["t"] for x in samples]
        i0, i1 = bisect.bisect_left(ts, t0), bisect.bisect_right(ts, t1) - 1
        dt = (ts[i1] - ts[i0]) / 1e9
        ent = [sum(samples[i1]["cpu"][c][k][0] - samples[i0]["cpu"][c][k][0] for c in SERVER) / dt for k in range(4)]
        rate = len(lat) / ((t1 - t0) / 1e9)
        slow = 100 * sum(1 for x in lat if x > a.slow) / max(1, len(lat))
        kcyc = rec["served"]["groups"]["s70"]["server"]["cycles_per_req"] / 1e3
        name = run.split("/")[-1]
        print(f"{name:34} {kcyc:6.1f} {rate / 1e3:5.0f}k {q(lat, .99):5.1f} {q(lat, .999):6.1f} {slow:6.3f}  {ent[0]:7.0f} {ent[1]:7.0f} {ent[2] + ent[3]:7.0f}  {ent[1] / rate:6.2f} {1e3 * (ent[2] + ent[3]) / rate:9.2f}")


if __name__ == "__main__":
    main()
