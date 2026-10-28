#!/usr/bin/env python3
"""The series of two soak runs (soak.py) side by side: asmalloc's and the reference's.

  summary.py ASM_DIR REF_DIR

Prints, in MiB unless said otherwise:
- the quiet points in groups of five cycles: the server's anonymous memory, its RSS, its
  cgroup's memory.current, the mappings, the background thread's share of a CPU over the
  second half of each quiet stretch, and asmalloc's count against mincore (memstat's `free`
  less its `retained`) at the stretch's last sample;
- the load phases in the same groups: the peak anonymous memory and AnonHugePages;
- the totals: phases, error counts, faults, CPU time.
"""
import json
import statistics
import sys

MIB = 1 << 20


def load(d, name):
    with open(f"{d}/{name}") as f:
        return [json.loads(line) for line in f if line.strip()]


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else float("nan")


def main():
    asm, ref = sys.argv[1], sys.argv[2]
    qa, qr = load(asm, "quiet.jsonl"), load(ref, "quiet.jsonl")
    pa, pr = load(asm, "phases.jsonl"), load(ref, "phases.jsonl")
    n = min(len(qa), len(qr))
    print(f"quiet points: {len(qa)} asmalloc, {len(qr)} reference; compared: {n}")
    print("\nAt the quiet points, medians over each group of five cycles (MiB):")
    print("| cycles | anon asm / je | RSS asm / je | cgroup asm / je | mappings asm / je |"
          " thread CPU share, max | count less mincore |")
    print("|---|---|---|---|---|---|---|")
    for g in range(0, n, 5):
        a, r = qa[g:min(g + 5, n)], qr[g:min(g + 5, n)]

        def roll(qs, key):
            return med(q["last"]["rollup"].get(key, 0) / 1024 for q in qs)

        def cg(qs):
            return med(q["last"].get("cg_current", 0) / MIB for q in qs)

        gaps = [(q["memstat"]["retained"] - q["memstat"]["free"]) / MIB
                for q in a if q.get("memstat")]
        share = max((q.get("bg_share") or 0) for q in a)
        print(f"| {g}-{g + len(a) - 1} | {roll(a, 'Anonymous'):.0f} / {roll(r, 'Anonymous'):.0f}"
              f" | {roll(a, 'Rss'):.0f} / {roll(r, 'Rss'):.0f} | {cg(a):.0f} / {cg(r):.0f}"
              f" | {med(q['last']['maps'] for q in a):.0f} / {med(q['last']['maps'] for q in r):.0f}"
              f" | {share * 100:.2f}% | {min(gaps, default=0):+.1f} to {max(gaps, default=0):+.1f} |")
    print("\nUnder load, the peaks of each group of five cycles (MiB):")
    print("| cycles | peak anon asm / je | peak AnonHugePages asm / je |")
    print("|---|---|---|")
    cycles = max((p["cycle"] for p in pa), default=-1) + 1
    for g in range(0, cycles, 5):
        a = [p for p in pa if g <= p["cycle"] < g + 5 and "peak_anon_kib" in p]
        r = [p for p in pr if g <= p["cycle"] < g + 5 and "peak_anon_kib" in p]
        if not a or not r:
            continue
        print(f"| {g}-{g + 4} | {max(p['peak_anon_kib'] for p in a) / 1024:.0f} /"
              f" {max(p['peak_anon_kib'] for p in r) / 1024:.0f} |"
              f" {max(p['peak_anonhuge_kib'] for p in a) / 1024:.0f} /"
              f" {max(p['peak_anonhuge_kib'] for p in r) / 1024:.0f} |")
    print("\nTotals:")
    for name, d, ps in (("asmalloc", asm, pa), ("reference", ref, pr)):
        series = load(d, "series.jsonl")
        first, last = series[0], series[-1]
        hours = (last["t"] - first["t"]) / 3600
        errs = sum(p.get("errors", 0) for p in ps)
        bad = [(i, p["kind"], p.get("errors", 0), p.get("reconnects", 0), p.get("unexpected", 0))
               for i, p in enumerate(ps)
               if p.get("errors", 0) or p.get("reconnects", 0) or p.get("unexpected", 0)]
        print(f"- {name}: {hours:.1f} h sampled, {len(ps)} load phases, {errs} errors"
              f" in {len(bad)} phases {bad[:8]}, minor faults {last['minflt'] - first['minflt']},"
              f" major {last['majflt'] - first['majflt']}, server CPU {last['cpu_ms'] / 3.6e6:.2f} h,"
              f" background thread {last['bg_cpu_ms'] / 1000:.1f} s,"
              f" mappings {min(s['maps'] for s in series)} to {max(s['maps'] for s in series)}")


if __name__ == "__main__":
    main()
