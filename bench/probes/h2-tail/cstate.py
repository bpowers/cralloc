#!/usr/bin/env python3
"""cstate.py OUT DUR [--cpus 0-3,8-11] [--ms 5]: samples each CPU's idle-state counters
(/sys/devices/system/cpu/cpuN/cpuidle/stateK/usage and time) every --ms for DUR seconds, with
CLOCK_MONOTONIC, into OUT as JSON lines {"t": ns, "cpu": {N: [[usage, time_us], ...]}}.

The kernel counts, per CPU, how many times each idle state was entered and the time spent in
it; nothing needs privileges or a tracepoint. Run it pinned off the measured CPUs.
"""
import argparse
import json
import time
from pathlib import Path


def cpus(spec):
    out = []
    for part in spec.split(","):
        a, _, b = part.partition("-")
        out.extend(range(int(a), int(b or a) + 1))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("dur", type=float)
    ap.add_argument("--cpus", default="0-3,8-11")
    ap.add_argument("--ms", type=float, default=5.0)
    a = ap.parse_args()
    root = Path("/sys/devices/system/cpu")
    files = {}
    for c in cpus(a.cpus):
        states = sorted((root / f"cpu{c}" / "cpuidle").glob("state*"), key=lambda p: int(p.name[5:]))
        files[c] = [(open(s / "usage", "rb", buffering=0), open(s / "time", "rb", buffering=0)) for s in states]
    end = time.monotonic() + a.dur
    with open(a.out, "w") as out:
        nxt = time.monotonic()
        while nxt < end:
            row = {}
            t = time.monotonic_ns()
            for c, fs in files.items():
                vals = []
                for u, tm in fs:
                    u.seek(0)
                    tm.seek(0)
                    vals.append([int(u.read()), int(tm.read())])
                row[c] = vals
            out.write(json.dumps({"t": t, "cpu": row}) + "\n")
            nxt += a.ms / 1e3
            d = nxt - time.monotonic()
            if d > 0:
                time.sleep(d)


if __name__ == "__main__":
    main()
