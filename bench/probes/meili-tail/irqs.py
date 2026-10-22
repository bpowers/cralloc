#!/usr/bin/env python3
"""Interrupt counts per CPU beside a session (docs/research/meili-tail.md): every PERIOD
seconds, the TLB-shootdown, function-call, rescheduling and timer interrupts of each CPU from
/proc/interrupts, with CLOCK_MONOTONIC in ns (the clock the load generator's records and its
`start_mono_ns` are on). One JSON line a sample: {"m": ns, "TLB": [...], "CAL": [...], ...},
indexed by CPU. Run pinned to a CPU the server and the load do not use; stop it with SIGTERM.

  irqs.py --out FILE [--period 0.1]
"""
import argparse
import json
import signal
import time

KEYS = ("TLB", "CAL", "RES", "LOC")


def read():
    out = {}
    with open("/proc/interrupts") as f:
        ncpu = len(f.readline().split())
        for line in f:
            name, _, rest = line.partition(":")
            name = name.strip()
            if name in KEYS:
                out[name] = [int(x) for x in rest.split()[:ncpu]]
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--period", type=float, default=0.1)
    a = ap.parse_args()
    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append(1))
    signal.signal(signal.SIGINT, lambda *_: stop.append(1))
    with open(a.out, "w", buffering=1 << 16) as f:
        nxt = time.monotonic()
        while not stop:
            s = read()
            s["m"] = time.monotonic_ns()
            f.write(json.dumps(s) + "\n")
            nxt += a.period
            time.sleep(max(0.0, nxt - time.monotonic()))


main()
