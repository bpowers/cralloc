#!/usr/bin/env python3
"""Summarizes a session of session.sh: the allocator's address-space calls (wl.rs) by kind, by
thread and by what the caller was doing, over each segment of the load plan; the calls that
take the lock for writing, their durations and the worst; what overlapped the long ones; and
how the slowest requests line up with them.

  wlsum.py OUTDIR TAG [--long-us 100] [--worst 15]
"""
import argparse
import json
import struct
from collections import defaultdict
from pathlib import Path

OPS = {1: "mprotect", 2: "dontneed", 3: "hugepage", 4: "nohugepage", 5: "madv-other", 6: "munmap",
       7: "mmap", 8: "mincore"}
WRITERS = {1, 3, 4, 6, 7}
CTX = {0: "-", 1: "commit", 2: "to-huge", 3: "pass", 4: "hold", 5: "growth", 6: "trim", 7: "move",
       8: "resize", 9: "free", 10: "alloc"}


def pct(v, p):
    if not v:
        return 0
    v = sorted(v)
    return v[min(len(v) - 1, int(len(v) * p))]


def load_wl(path):
    data = path.read_bytes()
    out = []
    for t0, w1, ln, w3, cpu, sw in struct.iter_unpack("<QQQQQQ", data[: len(data) // 48 * 48]):
        out.append((t0, w1 & 0xffffffff, w1 >> 32, ln, w3 & 0xff, (w3 >> 8) & 0xff, (w3 >> 16) << 12,
                    cpu, sw & 0xffffffff, sw >> 32))
    out.sort()
    return out


def kind_of(name):
    if name == "asmalloc":
        return "background"
    return "worker"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("tag")
    ap.add_argument("--long-us", type=float, default=100)
    ap.add_argument("--worst", type=int, default=15)
    a = ap.parse_args()
    d = Path(a.dir)
    recs = [json.loads(l) for l in open(d / f"{a.tag}.jsonl")]
    runs = [r for r in recs if r.get("served")]
    wldir = d / f"{a.tag}.wl"
    for wl in sorted(wldir.glob("*.wl")):
        pid = wl.stem
        names = {}
        tf = wldir / f"{pid}.threads"
        if tf.exists():
            for line in tf.read_text().splitlines():
                t, _, n = line.partition(" ")
                names[int(t)] = n
        calls = load_wl(wl)
        if not calls:
            continue
        print(f"== pid {pid}: {len(calls)} calls")
        # The plan's segments, on the generator's clock (CLOCK_MONOTONIC from start_mono_ns).
        gen = None
        for r in runs:
            g = (r.get("served") or {}).get("generator")
            if g:
                gen = g
        start = gen["start_mono_ns"] if gen else None
        win, segnames = {}, {}
        for i, sg in enumerate(gen["segments"] if gen else []):
            segnames[i] = sg["name"]
            win[i] = [start + sg["begin_ms"] * 1e6, start + sg["end_ms"] * 1e6, 0]
        raw = d / f"{a.tag}.raw" / f"{pid}.raw"
        reqs = []
        if raw.exists() and start:
            b = raw.read_bytes()
            for due, lat, seg, fl in struct.iter_unpack("<QQHH", b[: len(b) // 20 * 20]):
                reqs.append((start + due, lat, seg, fl))

        def seg_of(t):
            for s, (lo, hi, _) in win.items():
                if lo <= t <= hi:
                    return s
            return None

        # Calls by segment, op, thread kind and context.
        tab = defaultdict(lambda: [0, []])
        for t0, dur, tid, ln, op, ctx, addr, cpu, nv, niv in calls:
            s = seg_of(t0)
            key = (s, OPS.get(op, op), kind_of(names.get(tid, "?")), CTX.get(ctx, ctx))
            tab[key][0] += 1
            tab[key][1].append(dur)
        print(f"  {'segment':>10} {'op':>10} {'thread':>10} {'doing':>8} {'calls':>8} {'/s':>8} "
              f"{'p50 us':>8} {'p99 us':>8} {'max us':>9}")
        for key in sorted(tab, key=lambda k: (str(k[0]), k[1], k[2], k[3])):
            n, durs = tab[key]
            s = key[0]
            secs = (win[s][1] - win[s][0]) / 1e9 if s in win else None
            name = segnames.get(s, "outside") if s is not None else "outside"
            rate = f"{n / secs:8.1f}" if secs else f"{'':>8}"
            print(f"  {name:>10} {key[1]:>10} {key[2]:>10} {key[3]:>8} {n:8d} {rate} "
                  f"{pct(durs, .5) / 1e3:8.1f} {pct(durs, .99) / 1e3:8.1f} {max(durs) / 1e3:9.1f}")
        # The worst writers, and what was in flight beside them.
        writers = sorted((c for c in calls if c[4] in WRITERS), key=lambda c: -c[1])[: a.worst]
        print(f"  worst writers:")
        for t0, dur, tid, ln, op, ctx, addr, cpu, nv, niv in writers:
            over = [c for c in calls if c is not None and c[0] < t0 + dur and c[0] + c[1] > t0
                    and not (c[0] == t0 and c[2] == tid)]
            s = seg_of(t0)
            desc = ", ".join(f"{OPS.get(c[4])}/{CTX.get(c[5])}/{kind_of(names.get(c[2], '?'))} "
                             f"{c[3] >> 10}K {c[1] / 1e3:.0f}us" for c in over[:6])
            print(f"    {dur / 1e3:9.1f} us {OPS.get(op)} {ln >> 10}K by {names.get(tid, tid)} ({CTX.get(ctx)}) "
                  f"in {segnames.get(s, 'outside') if s is not None else 'outside'}; beside: {desc or 'nothing of ours'}")
        # The slowest requests of each measured segment against the long calls.
        if reqs:
            long = [c for c in calls if c[1] >= a.long_us * 1e3]
            longw = [c for c in long if c[4] in WRITERS]
            by = defaultdict(list)
            for r in reqs:
                by[r[2]].append(r)
            for s, rs in sorted(by.items()):
                name = segnames.get(s, str(s))
                if name.startswith("_") or len(rs) < 1000:
                    continue
                rs.sort(key=lambda r: -r[1])
                top = rs[: max(1, len(rs) // 1000)]
                p999 = top[-1][1]

                def hit(r, cs):
                    lo, hi = r[0], r[0] + r[1]
                    return any(c[0] < hi and c[0] + c[1] > lo for c in cs)
                nl = sum(hit(r, long) for r in top)
                nw = sum(hit(r, longw) for r in top)
                print(f"  {name}: {len(rs)} requests, p99.9 {p999 / 1e3:.0f} us; of the slowest 0.1% ({len(top)}), "
                      f"{nl} overlap a call of ours >= {a.long_us:.0f} us, {nw} a writer that long")


if __name__ == "__main__":
    main()
