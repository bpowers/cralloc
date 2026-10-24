#!/usr/bin/env python3
"""show.py RUN [--slow US] [--n N] [--fast]: print what every server CPU and the generator's
CPUs did around N slow requests (or, with --fast, N ordinary ones), from trace.sh's
switch-event recording: each run of a thread that overlaps the request, in us from its due
time, with the requests in flight on the same generator thread's schedule beside it.
"""
import argparse
import json
import random
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from timeline import CLIENT_CPUS, SERVER_CPUS, switches


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("run")
    ap.add_argument("--slow", type=float, default=42.0)
    ap.add_argument("--n", type=int, default=6)
    ap.add_argument("--fast", action="store_true")
    ap.add_argument("--seed", type=int, default=1)
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
    reqs = [(base + d, l) for d, l, s, f in struct.iter_unpack("<QQHH", raw.read_bytes())
            if not f & 4 and names[s].startswith(("s70", "t")) and t_lo < base + d and base + d + l < t_hi]
    pick = [r for r in reqs if (r[1] > a.slow * 1e3) != a.fast]
    random.seed(a.seed)
    for t0, l in sorted(random.sample(pick, min(a.n, len(pick)))):
        lo, hi = t0 - 30_000, t0 + l + 10_000
        print(f"\nrequest due {t0 / 1e9:.6f} ({t0 % 1_000_000 / 1e3:.0f} us into the ms), latency {l / 1e3:.1f} us")
        near = [(d - t0, x) for d, x in reqs if lo <= d <= hi]
        print("  requests due nearby (us from due, latency): " + ", ".join(f"{d / 1e3:+.0f}:{x / 1e3:.0f}" for d, x in near))
        for c in SERVER_CPUS + CLIENT_CPUS:
            segs = []
            for s, e, pid, tid, comm in runs.get(c, []):
                if e > lo and s < hi:
                    who = "idle" if pid == 0 else (f"w{tid - spid}" if pid == spid else (comm if c in SERVER_CPUS or pid != 0 else comm))
                    if c in CLIENT_CPUS and pid != 0 and pid != spid:
                        who = "gen" if "loadgen" in comm else comm
                    segs.append(f"{who}[{(s - t0) / 1e3:+.1f},{(e - t0) / 1e3:+.1f}]")
            print(f"  cpu{c:<2} " + " ".join(segs))


if __name__ == "__main__":
    main()
