#!/usr/bin/env python3
"""procsum.py RUN... [--seg s70]: per trace.sh run, over one segment of the plan: server cycles
and instructions a request (benchrun's), and from procs.py's samples the server's read- and
write-type system calls, bytes written, and times its threads were scheduled in, a request;
and from cstate.py's, the server CPUs' idle entries a request.
"""
import argparse
import bisect
import json
import struct
from pathlib import Path

SERVER = ("0", "1", "2", "3")


def at(samples, t):
    ts = [x["t"] for x in samples]
    return samples[min(len(samples) - 1, bisect.bisect_left(ts, t))]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("runs", nargs="+")
    ap.add_argument("--seg", default="s70")
    a = ap.parse_args()
    print(f"{'run':30} {'kcyc':>6} {'kins':>6} {'p99':>5} {'reads':>6} {'writes':>6} {'wB':>6} {'sched':>6} {'idle':>5}  per request")
    for run in a.runs:
        rec = next(json.loads(l) for l in open(run + ".jsonl") if '"type":"run"' in l.replace(" ", ""))
        base = rec["served"]["generator"]["start_mono_ns"]
        segs = rec["served"]["generator"]["segments"]
        g = rec["served"]["groups"][a.seg]
        s = next(x for x in segs if x["name"] == a.seg)
        # The middle of the segment, a second in from each end, so the 100 ms samples sit inside.
        t0, t1 = base + (s["begin_ms"] + 1000) * 1e6, base + (s["end_ms"] - 1000) * 1e6
        want = {i for i, x in enumerate(segs) if x["name"] == a.seg}
        n = sum(1 for d, l, sg, f in struct.iter_unpack("<QQHH", next(Path(run + ".raw").glob("*.raw")).read_bytes())
                if sg in want and t0 <= base + d < t1)
        pr = [json.loads(x) for x in open(run + ".procs.jsonl")]
        p0, p1 = at(pr, t0), at(pr, t1)
        frac = (p1["t"] - p0["t"]) / (t1 - t0)
        nn = n * frac
        cs = [json.loads(x) for x in open(run + ".cstate.jsonl")]
        c0, c1 = at(cs, p0["t"]), at(cs, p1["t"])
        idle = sum(c1["cpu"][c][k][0] - c0["cpu"][c][k][0] for c in SERVER for k in range(4))
        d = lambda k: (p1["io"][k] - p0["io"][k]) / nn
        sched = (p1["sched"][2] - p0["sched"][2]) / nn
        sv = g["server"]
        print(f"{run.split('/')[-1]:30} {sv['cycles_per_req'] / 1e3:6.1f} {sv['instructions_per_req'] / 1e3:6.1f} "
              f"{g['lat_us']['p99']:5.1f} {d('syscr'):6.2f} {d('syscw'):6.2f} {d('wchar'):6.0f} {sched:6.2f} {idle / nn:5.2f}")


if __name__ == "__main__":
    main()
