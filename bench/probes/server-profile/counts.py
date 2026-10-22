#!/usr/bin/env python3
"""The allocation profile of a server from a `count.py` run: calls per request by size class,
which path served them, realloc, frees on another CPU or thread, lifetimes, large objects, and
the allocator's system calls.

  counts.py DIR [--json OUT]

Counters (counters.json) give per-request rates over the interval between two reads of the
70% segment. The record (trace-PID.bin) gives distributions: its window holds every call, and
its tail every free of a block whose address hashes to one in 64 (the same hash the wrapper
uses) and every large call, so a block allocated in the window with such an address is
followed to its free, however long it lives (up to the tail's end).
"""
import argparse
import bisect
import collections
import json
import math
import struct
import sys
from pathlib import Path

ROW, ROWS, BG = 64, 65, 64
NAMES = {0: "slow alloc", 1: "slow free", 2: "alloc: spare swap", 3: "alloc: depot pop", 4: "alloc: fill",
         5: "fill objects", 6: "free: spare swap", 7: "free: depot pop (empty)", 8: "free: new magazine",
         9: "centred swap", 10: "push to shared stack", 11: "launder", 12: "laundered objects", 13: "heartbeat",
         14: "large alloc", 15: "large alloc bytes", 16: "large free", 17: "resize", 18: "resize small in place",
         19: "resize large in place", 20: "resize move", 21: "resize move bytes", 22: "served from source (no CPU)",
         23: "slow events", 24: "mprotect", 25: "mprotect bytes", 26: "madvise", 27: "madvise bytes", 28: "munmap",
         29: "membarrier", 30: "futex wait", 31: "futex wake", 32: "mincore", 33: "maintain round",
         36: "large move"}
for k in range(20):
    NAMES[40 + k] = f"large alloc 2^{k + 15}"
CALLS = ["alloc", "free", "realloc", "alloc_zeroed", "large alloc", "large free", "large realloc", "bytes allocated"]
LARGE = 32 * 1024
REC = struct.Struct("<QQQIIBBHB3x")
OPS = {0: "alloc", 1: "free", 2: "realloc", 3: "zeroed"}


def class_of(size, align_log2):
    """asmalloc's size class of a Rust layout: rounded to its alignment, multiples of 8 to 64,
    then four classes per doubling."""
    align = 1 << align_log2
    r = max(1, -(-size // align) * align)
    if r <= 64:
        return max(8, -(-r // 8) * 8)
    p = 1 << ((r - 1).bit_length() - 1)
    step = p // 4
    return -(-r // step) * step


def followed(ptr):
    return ((ptr >> 4) * 0x9E3779B97F4A7C15 & 0xFFFFFFFFFFFFFFFF) >> 58 == 0


def load_trace(path):
    data = open(path, "rb").read()
    assert data[:8] == b"PROF0001", path
    chunks, = struct.unpack_from("<Q", data, 8)
    hz, = struct.unpack_from("<d", data, 16)
    dropped, = struct.unpack_from("<Q", data, 24)
    off = 32
    recs = []
    for _ in range(chunks):
        c, n = struct.unpack_from("<QQ", data, off)
        off += 16
        for i in range(n):
            tsc, ptr, old, size, old_size, op, flags, cpu, al = REC.unpack_from(data, off + 40 * i)
            recs.append((tsc, c, cpu, op, flags, ptr, size, al, old, old_size))
        off += 40 * n
    recs.sort()
    return recs, hz, dropped


def q(xs, ps=(0.5, 0.9, 0.99)):
    if not xs:
        return []
    xs = sorted(xs)
    return [xs[min(len(xs) - 1, int(p * len(xs)))] for p in ps]


def fmt_t(s):
    if s < 1e-3:
        return f"{s * 1e6:.0f} us"
    if s < 1:
        return f"{s * 1e3:.1f} ms"
    return f"{s:.2f} s"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--json")
    ap.add_argument("--no-trace", action="store_true")
    a = ap.parse_args()
    d = Path(a.dir)
    meta = json.loads((d / "meta.json").read_text())
    cs = json.loads((d / "counters.json").read_text())
    rate = meta["rate"]
    s1, s2 = cs["s1"], cs["s2"]
    dt = s2["t"] - s1["t"]
    reqs = rate * dt
    out = {"job": meta["job"]}
    print(f"{meta['job']}: counters over {dt:.2f} s at {rate}/s = {reqs:,.0f} requests")
    calls = [sum(s2["calls"][l * 8 + i] - s1["calls"][l * 8 + i] for l in range(64)) for i in range(8)]
    out["calls_per_request"] = {CALLS[i]: calls[i] / reqs for i in range(8)}
    print("calls a request:", ", ".join(f"{CALLS[i]} {calls[i] / reqs:.2f}" for i in range(8)))
    prof = [sum(s2["prof"][r * ROW + i] - s1["prof"][r * ROW + i] for r in range(ROWS - 1)) for i in range(ROW)]
    bgp = [s2["prof"][BG * ROW + i] - s1["prof"][BG * ROW + i] for i in range(ROW)]
    small_allocs = calls[0] + calls[3]
    small_frees = calls[1]
    print(f"slow paths a request: alloc {prof[0] / reqs:.4f} ({100 * prof[0] / max(1, small_allocs):.3f}% of small "
          f"allocations), free {prof[1] / reqs:.4f} ({100 * prof[1] / max(1, small_frees):.3f}% of small frees)")
    lam = (prof[0] + prof[1] + prof[14] + prof[16]) / reqs
    print(f"slow paths and large calls a request: {lam:.4f}; if independent, "
          f"{100 * (1 - math.exp(-lam)):.2f}% of requests meet at least one")
    out["slow_per_request"] = lam
    out["counters_per_request"] = {NAMES[i]: prof[i] / reqs for i in NAMES if prof[i]}
    out["background_per_second"] = {NAMES[i]: bgp[i] / dt for i in NAMES if bgp[i]}
    for i in sorted(NAMES):
        if prof[i]:
            print(f"  {NAMES[i]:30} {prof[i] / reqs:14.5f} a request")
    if any(bgp):
        print("background thread, a second:", ", ".join(f"{NAMES[i]} {bgp[i] / dt:.1f}" for i in sorted(NAMES) if bgp[i]))

    traces = [] if a.no_trace else sorted(d.glob("trace-*.bin"))
    if not traces:
        print("no trace")
        return
    recs, hz, dropped = load_trace(traces[0])
    print(f"\ntrace: {len(recs):,} records, TSC {hz / 1e9:.3f} GHz, {dropped:,} dropped")
    small_alloc_t = [r[0] for r in recs if r[3] in (0, 3) and r[6] <= LARGE]
    if not small_alloc_t:
        print("no window")
        return
    w0, w1 = recs[0][0], max(small_alloc_t)
    wsec = (w1 - w0) / hz
    wreqs = rate * wsec
    tail_end = recs[-1][0]
    window = [r for r in recs if r[0] <= w1]
    print(f"window {wsec * 1e3:.0f} ms ({wreqs:,.0f} requests), tail to {(tail_end - w0) / hz:.1f} s")

    # Calls by class in the window.
    by_class = collections.Counter()
    slow_by_op = collections.Counter()
    op_n = collections.Counter()
    for tsc, c, cpu, op, fl, ptr, size, al, old, old_size in window:
        name = OPS[op]
        op_n[name] += 1
        if fl & 1:
            slow_by_op[name] += 1
        if op in (0, 3):
            by_class[class_of(size, al) if size <= LARGE else "large"] += 1
    print("window calls a request:", ", ".join(f"{k} {v / wreqs:.2f}" for k, v in op_n.most_common()))
    print("slow share by call (window, flag):",
          ", ".join(f"{k} {100 * slow_by_op[k] / v:.3f}%" for k, v in op_n.items()))
    tot = sum(by_class.values())
    print("allocations a request by class (window), largest first:")
    rows = sorted(by_class.items(), key=lambda kv: -kv[1])
    out["alloc_by_class"] = {str(k): v / wreqs for k, v in rows}
    for k, v in rows[:20]:
        print(f"  {str(k):>7} B {v / wreqs:9.2f}  {100 * v / tot:5.1f}%")

    # realloc.
    rs = [r for r in window if r[3] == 2]
    if rs:
        inplace = [r for r in rs if r[5] == r[8]]
        moved = [r for r in rs if r[5] != r[8]]
        grow = [r for r in rs if r[6] > r[9]]
        ratios = [r[6] / max(1, r[9]) for r in moved]
        copied = sum(min(r[6], r[9]) for r in moved)
        print(f"realloc: {len(rs) / wreqs:.2f} a request, {100 * len(inplace) / len(rs):.1f}% in place, "
              f"{100 * len(grow) / len(rs):.1f}% grow; moved: {len(moved) / wreqs:.2f} a request, new/old size "
              f"median {q(ratios)[0] if ratios else 0:.2f}, {copied / wreqs:,.0f} bytes copied a request")
        old_cls = collections.Counter((class_of(r[9], r[7]) if r[9] <= LARGE else "large",
                                       class_of(r[6], r[7]) if r[6] <= LARGE else "large") for r in rs)
        print("  commonest (old class -> new class):",
              ", ".join(f"{a}->{b}: {v / wreqs:.2f}" for (a, b), v in old_cls.most_common(8)))
        out["realloc"] = {"per_request": len(rs) / wreqs, "in_place": len(inplace) / len(rs),
                          "moved_per_request": len(moved) / wreqs, "bytes_copied_per_request": copied / wreqs}

    lifetimes(recs, w1, hz, wreqs, out)
    if a.json:
        Path(a.json).write_text(json.dumps(out, indent=1))


def lifetimes(recs, w1, hz, wreqs, out):
    """Pairs every block allocated in the window with its free, and reports lifetimes and where
    the free happened, over all window blocks (biased to short lives: a block that outlives the
    window is seen only if followed) and over the followed one in 64 (unbiased up to the tail)."""
    live = {}
    done = []
    for tsc, c, cpu, op, fl, ptr, size, al, old, old_size in recs:
        if op in (0, 3) and tsc <= w1:
            live[ptr] = (tsc, c, cpu, size, al, followed(ptr))
        elif op == 1:
            st = live.pop(ptr, None)
            if st:
                done.append((st, tsc, c, cpu))
        elif op == 2:
            st = live.pop(old, None)
            if st and old != ptr:
                done.append((st, tsc, c, cpu))
            elif st:
                live[ptr] = st
            if old != ptr and tsc <= w1:
                live[ptr] = (tsc, c, cpu, size, al, followed(ptr))
    # Window blocks: allocated in the window. Followed: their address hashes in.
    n_window = sum(1 for r in recs if r[3] in (0, 3) and r[0] <= w1) + sum(1 for r in recs if r[3] == 2 and r[0] <= w1 and r[5] != r[8])
    fol_alloc = sum(1 for r in recs if r[0] <= w1 and r[3] in (0, 3, 2) and followed(r[5]) and (r[3] != 2 or r[5] != r[8]))
    fol_done = [x for x in done if x[0][5]]
    fol_live = sum(1 for st in live.values() if st[5])
    def stats(xs, label, total=None):
        if not xs:
            return {}
        life = [(t - st[0]) / hz for st, t, c, cpu in xs]
        other_cpu = sum(1 for st, t, c, cpu in xs if cpu != st[2]) / len(xs)
        other_thread = sum(1 for st, t, c, cpu in xs if c != st[1]) / len(xs)
        ql = q(life)
        line = (f"  {label}: {len(xs):,} freed, lifetime p50 {fmt_t(ql[0])}, p90 {fmt_t(ql[1])}, p99 {fmt_t(ql[2])}; "
                f"freed on another CPU {100 * other_cpu:.1f}%, another thread {100 * other_thread:.1f}%")
        print(line)
        return {"freed": len(xs), "life_p50": ql[0], "life_p90": ql[1], "life_p99": ql[2],
                "other_cpu": other_cpu, "other_thread": other_thread}
    print("frees of blocks allocated in the window:")
    out["pairs_all"] = stats(done, "all blocks (short lives over-represented)")
    out["pairs_followed"] = stats(fol_done, "followed blocks (1 in 64 by address)")
    if fol_alloc:
        print(f"  followed blocks not freed by the tail's end: {fol_live} of {fol_alloc} "
              f"({100 * fol_live / fol_alloc:.1f}%)")
        out["followed_unfreed"] = fol_live / fol_alloc
    # By class, followed blocks only.
    byc = collections.defaultdict(list)
    for st, t, c, cpu in fol_done:
        size, al = st[3], st[4]
        byc[class_of(size, al) if size <= LARGE else "large"].append((st, t, c, cpu))
    print("  followed blocks by class (most freed first): class, freed, other CPU, p50 life")
    for k, xs in sorted(byc.items(), key=lambda kv: -len(kv[1]))[:12]:
        life = q([(t - st[0]) / hz for st, t, c, cpu in xs])[0]
        oc = sum(1 for st, t, c, cpu in xs if cpu != st[2]) / len(xs)
        print(f"    {str(k):>7} {len(xs):7} {100 * oc:6.1f}% {fmt_t(life):>10}")
    # Large objects: every large call is recorded in window and tail.
    big = [r for r in recs if (r[3] in (0, 3, 1) and r[6] > LARGE) or (r[3] == 2 and (r[6] > LARGE or r[9] > LARGE))]
    if big:
        span = (recs[-1][0] - recs[0][0]) / hz
        allocs = [r for r in big if r[3] in (0, 3)]
        frees = [r for r in big if r[3] == 1]
        reall = [r for r in big if r[3] == 2]
        print(f"large calls over {span:.1f} s: {len(allocs)} allocs, {len(frees)} frees, {len(reall)} reallocs "
              f"({len(allocs) / span:.1f}, {len(frees) / span:.1f}, {len(reall) / span:.1f} a second)")
        sizes = collections.Counter(1 << (r[6] - 1).bit_length() for r in allocs)
        print("  alloc sizes (rounded up to a power of two):", ", ".join(f"{k >> 10} KiB: {v}" for k, v in sorted(sizes.items())))
        lv = {}
        lt = []
        for r in sorted(big):
            if r[3] in (0, 3):
                lv[r[5]] = r
            elif r[3] == 1 and r[5] in lv:
                s = lv.pop(r[5])
                lt.append(((r[0] - s[0]) / hz, s[1] == r[1], s[2] == r[2], s[6]))
            elif r[3] == 2:
                s = lv.pop(r[8], None)
                if s:
                    lt.append(((r[0] - s[0]) / hz, s[1] == r[1], s[2] == r[2], s[6]))
                lv[r[5]] = r
        if lt:
            ql = q([x[0] for x in lt])
            print(f"  {len(lt)} large blocks seen allocated and freed: lifetime p50 {fmt_t(ql[0])}, p90 {fmt_t(ql[1])}, "
                  f"p99 {fmt_t(ql[2])}; freed by the allocating thread {100 * sum(x[1] for x in lt) / len(lt):.0f}%, "
                  f"on the allocating CPU {100 * sum(x[2] for x in lt) / len(lt):.0f}%")
        out["large"] = {"allocs_per_s": len(allocs) / span, "frees_per_s": len(frees) / span,
                        "reallocs_per_s": len(reall) / span, "sizes": {str(k): v for k, v in sizes.items()},
                        "pairs": len(lt)}


if __name__ == "__main__":
    main()
