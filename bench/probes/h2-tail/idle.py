#!/usr/bin/env python3
"""idle.py RUN [--slow US]: the server CPUs' idle periods and each request's wake-up, from
trace.sh's switch events.

- Idle periods on CPUs 0-3 (idle runs between two server-thread runs), as a histogram: the
  idle governor picks a state by how long it expects the CPU to stay idle, so the lengths
  say which states are in play (C1 needs 2 us of residency on this host, C2 36 us).
- Per request, the first server thread to come on a CPU after the request came due (on a CPU
  idle when it came due): how long after due, and how long that CPU had been idle by then.
  Printed as histograms for the slow requests and for all.
"""
import argparse
import bisect
import json
import struct
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from timeline import SERVER_CPUS, switches


def hist(xs, step, top):
    c = Counter(min(int(x // step) * step, top) for x in xs)
    return ", ".join(f"{k:g}:{v}" for k, v in sorted(c.items()))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("run")
    ap.add_argument("--slow", type=float, default=42.0)
    a = ap.parse_args()
    run = Path(a.run)
    rec = next(json.loads(l) for l in open(str(run) + ".jsonl") if '"type":"run"' in l.replace(" ", ""))
    base = rec["served"]["generator"]["start_mono_ns"]
    names = [s["name"] for s in rec["served"]["generator"]["segments"]]
    raw = next(Path(str(run) + ".raw").glob("*.raw"))
    spid = int(raw.stem)
    runs = switches(str(run) + ".srv.data")
    t_lo = max(rs[0][0] for rs in runs.values() if rs)
    t_hi = min(rs[-1][1] for rs in runs.values() if rs)
    idle_lens = []
    # per CPU: (switch-in time of a server thread, length of the idle run before it)
    wake = {}
    for c in SERVER_CPUS:
        rs = runs.get(c, [])
        w = []
        for prev, cur in zip(rs, rs[1:]):
            if prev[2] == 0 and cur[2] == spid:
                idle_lens.append((prev[1] - prev[0]) / 1e3)
                w.append((cur[0], (prev[1] - prev[0]) / 1e3, prev[0]))
        wake[c] = w
    wt = {c: [x[0] for x in w] for c, w in wake.items()}
    print(f"server CPUs' idle periods before a server thread, us: {hist(idle_lens, 5, 200)}")
    out = {"slow": ([], []), "all": ([], [])}
    for d, l, s, f in struct.iter_unpack("<QQHH", raw.read_bytes()):
        if f & 4 or not names[s].startswith(("s70", "t")):
            continue
        t0 = base + d
        if t0 < t_lo or t0 + l > t_hi:
            continue
        best = None
        for c in SERVER_CPUS:
            i = bisect.bisect_right(wt[c], t0)
            if i < len(wake[c]):
                tw, il, idle_from = wake[c][i]
                # The CPU must have been idle when the request came due.
                if idle_from <= t0 and (best is None or tw < best[0]):
                    best = (tw, il)
        if best is None:
            continue
        delay, il = (best[0] - t0) / 1e3, best[1]
        for k in ("all",) + (("slow",) if l > a.slow * 1e3 else ()):
            out[k][0].append(delay)
            out[k][1].append(il)
    for k, (ds, ils) in out.items():
        print(f"{k}: {len(ds)} requests; first wake after due, us: {hist(ds, 2, 60)}")
        print(f"{'':6}idle length of that CPU at the wake, us: {hist(ils, 10, 300)}")


if __name__ == "__main__":
    main()
