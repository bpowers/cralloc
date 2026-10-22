#!/usr/bin/env python3
"""Meilisearch's tail at 70%, by allocator, from sessions of session.sh
(docs/research/meili-tail.md).

  tail.py [--ref NAME] [--seg s70] [--from-s 0] [--to-s 1e9] [--boot 4000] DIR...

Reads every session in each DIR (TAG.jsonl, with TAG.raw/, TAG.watch.jsonl and
TAG.irqs.jsonl beside it). A session is a round: each allocator once, in a random order.
For each valid run, from the load generator's record of every request due in the segment
(optionally only those due FROM_S to TO_S seconds into it): its p50, p99 and p99.9, and from
benchrun's counters over the segment, server cycles and page faults a request; from the
watcher, the allocator thread's wake-ups and CPU time a second; from irqs.py, TLB-shootdown
interrupts a second on each server CPU.

Prints, per allocator: the median and range over runs of each run's quantiles, the quantiles
of all its requests pooled, and the means of the rest. Then each allocator against REF: the
mean over rounds of the per-round difference in each run quantile, with a 95% interval from
resampling rounds; and the pooled quantiles' ratio, with a 95% interval from resampling
rounds (each resample pools the chosen rounds' requests). Pooled quantiles come from a
histogram of 0.1% bins.
"""
import argparse
import json
import math
import random
import statistics as st
import struct
from bisect import bisect_left
from collections import defaultdict
from pathlib import Path

BIN = math.log(1.001)
LO = 1e4  # ns


def bin_of(ns):
    return 0 if ns <= LO else int(math.log(ns / LO) / BIN) + 1


def bin_ns(b):
    return LO * math.exp((b - 0.5) * BIN) if b else LO


def hist_quantile(h, n, q):
    """Nearest-rank quantile of a histogram {bin: count} holding n values, in ns."""
    rank = max(1, math.ceil(q * n))
    acc = 0
    for b in sorted(h):
        acc += h[b]
        if acc >= rank:
            return bin_ns(b)
    return float("nan")


def pct(sorted_v, q):
    rank = max(1, math.ceil(q * len(sorted_v)))
    return sorted_v[min(rank, len(sorted_v)) - 1]


def read_raw(path):
    data = Path(path).read_bytes()
    for off in range(0, len(data), 20):
        yield struct.unpack_from("<QQHH", data, off)


def load_session(jpath, a):
    tag = jpath.name[: -len(".jsonl")]
    d = jpath.parent
    recs = [json.loads(l) for l in open(jpath)]
    runs = [r for r in recs if r.get("type") == "run"]
    procs, gens, samples = [], [], defaultdict(list)
    wp = d / f"{tag}.watch.jsonl"
    if wp.exists():
        for l in open(wp):
            w = json.loads(l)
            if w["type"] == "proc":
                procs.append(w)
            elif w["type"] == "gen":
                gens.append(w)
            elif w["type"] == "s":
                samples[w["pid"]].append(w)
    irqs = []
    ip = d / f"{tag}.irqs.jsonl"
    if ip.exists():
        irqs = [json.loads(l) for l in open(ip)]
    raws = {int(p.stem): p for p in (d / f"{tag}.raw").glob("*.raw")}
    out = []
    for r in runs:
        if not r.get("valid"):
            print(f"# {tag}: {r['allocator']} invalid: {r.get('problems')}")
            continue
        build = r["argv"][0].split("/target/")[1].split("/")[0]
        # The server is the watched process of this build that started first after the run.
        cand = [p for p in procs if p["exe"] == r["argv"][0] or f"/target/{build}/" in p["exe"]]
        cand = [p for p in cand if p["start"] >= r["started"] - 1]
        proc = min(cand, key=lambda p: p["start"], default=None)
        if proc is None or proc["pid"] not in raws:
            print(f"# {tag}: {r['allocator']}: no request record")
            continue
        gen = r["served"]["generator"]
        segs = gen["segments"]
        idx = {i for i, s in enumerate(segs) if s["name"] == a.seg}
        begin = min(segs[i]["begin_ms"] for i in idx)
        end = max(segs[i]["end_ms"] for i in idx)
        lo_ns = (begin + a.from_s * 1e3) * 1e6
        hi_ns = min(end, begin + a.to_s * 1e3) * 1e6
        lat = sorted(l for due, l, seg, fl in read_raw(raws[proc["pid"]])
                     if seg in idx and not fl & 2 and lo_ns <= due < hi_ns)
        g = r["served"]["groups"][a.seg]
        run = {
            "alloc": r["allocator"], "round": tag, "n": len(lat),
            "p50": pct(lat, .5) / 1e6, "p90": pct(lat, .9) / 1e6, "p99": pct(lat, .99) / 1e6,
            "p999": pct(lat, .999) / 1e6, "max": lat[-1] / 1e6, "mean": sum(lat) / len(lat) / 1e6,
            "kcycles": g["server"]["cycles_per_req"] / 1e3, "faults": g["server"]["faults_per_req"],
            "hist": defaultdict(int),
            # The slowest 5%, slowest first, for the pooled tail quantiles.
            "top": [v / 1e6 for v in reversed(lat[-max(1, len(lat) // 20):])],
        }
        for v in lat:
            run["hist"][bin_of(v)] += 1
        # The segment in wall time (the watcher) and monotonic time (irqs.py).
        gstart = min((x["start"] for x in gens if x["start"] >= proc["start"]), default=None)
        sm = samples.get(proc["pid"], [])
        if gstart is not None and sm:
            w0, w1 = gstart + begin / 1e3, gstart + end / 1e3
            inside = [s for s in sm if w0 <= s["t"] <= w1]
            if len(inside) >= 2:
                f, l = inside[0], inside[-1]
                secs = l["t"] - f["t"]
                vol = cpu = 0
                for tid, (v1, _, c1) in l["threads"].items():
                    if tid in f["threads"]:
                        v0, _, c0 = f["threads"][tid]
                        vol += v1 - v0
                        cpu += c1 - c0
                run["wakes"] = vol / secs
                run["thread_ms"] = cpu * 10 / secs  # clock ticks of 10 ms
        if irqs:
            m0, m1 = gen["start_mono_ns"] + begin * 1e6, gen["start_mono_ns"] + end * 1e6
            inside = [s for s in irqs if m0 <= s["m"] <= m1]
            if len(inside) >= 2:
                f, l = inside[0], inside[-1]
                secs = (l["m"] - f["m"]) / 1e9
                cpus = [int(c) for c in r["cpus"].split(",")]
                run["tlb"] = sum(l["TLB"][c] - f["TLB"][c] for c in cpus) / len(cpus) / secs
                run["cal"] = sum(l["CAL"][c] - f["CAL"][c] for c in cpus) / len(cpus) / secs
                # Bursts: the shootdowns one server CPU took in a 100 ms sample, p99 and max
                # over every CPU's samples.
                per = sorted(b["TLB"][c] - x["TLB"][c] for x, b in zip(inside, inside[1:]) for c in cpus)
                run["tlb100_p99"] = pct(per, .99)
                run["tlb100_max"] = per[-1]
        out.append(run)
    return out


def boot_ci(values, stat, reps, rng):
    est = []
    for _ in range(reps):
        s = [values[rng.randrange(len(values))] for _ in values]
        est.append(stat(s))
    est.sort()
    return est[int(.025 * reps)], est[int(.975 * reps) - 1]


def pooled_tail(runs, q):
    """The pooled quantile q (0.95 or above) of the runs' requests, exact, in ms."""
    n = sum(r["n"] for r in runs)
    k = n - max(1, math.ceil(q * n)) + 1
    top = sorted((v for r in runs for v in r["top"]), reverse=True)
    if k > len(top) or any(len(r["top"]) < r["n"] // 20 for r in runs):
        raise SystemExit("pooled_tail: kept too few of the slowest requests")
    v = top[k - 1]
    if any(r["top"][-1] > v for r in runs):
        raise SystemExit("pooled_tail: a run's tail is heavier than the 5% kept")
    return v


def pooled(runs, q):
    if q >= 0.95:
        return pooled_tail(runs, q)
    h, n = defaultdict(int), 0
    for r in runs:
        for b, c in r["hist"].items():
            h[b] += c
        n += r["n"]
    return hist_quantile(h, n, q) / 1e6


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--ref", default="asmalloc-parent")
    ap.add_argument("--seg", default="s70")
    ap.add_argument("--from-s", type=float, default=0)
    ap.add_argument("--to-s", type=float, default=1e9)
    ap.add_argument("--boot", type=int, default=4000)
    ap.add_argument("--runs", action="store_true", help="print every run")
    a = ap.parse_args()
    runs = []
    for d in a.dirs:
        for j in sorted(Path(d).glob("*.jsonl")):
            if j.name.count(".") == 1:
                runs += load_session(j, a)
    by = defaultdict(list)
    for r in runs:
        by[r["alloc"]].append(r)
    if a.runs:
        for r in sorted(runs, key=lambda r: (r["round"], r["alloc"])):
            print(f"{r['round']:>10} {r['alloc']:<16} p50 {r['p50']:.2f} p99 {r['p99']:.1f} p99.9 {r['p999']:.1f} "
                  f"max {r['max']:.0f} kc {r['kcycles']:.0f} flt {r['faults']:.1f} "
                  f"wake {r.get('wakes', float('nan')):.1f} thr {r.get('thread_ms', float('nan')):.1f} "
                  f"tlb {r.get('tlb', float('nan')):.0f}")
    print(f"\nsegment {a.seg}, requests due {a.from_s:g} s to {min(a.to_s, 1e9):g} s into it\n")
    print("| allocator | runs | requests | p50 ms: median of runs (range) | p99 | p99.9 | pooled p50 / p99 / p99.9 | "
          "kcycles a request | faults a request | thread wake-ups / CPU ms a second | TLB shootdowns a second a CPU |")
    print("|---|---:|---:|---|---|---|---|---:|---:|---|---:|")
    order = sorted(by, key=lambda k: (k != a.ref, k))
    for k in order:
        v = by[k]
        def col(f, nd):
            xs = sorted(r[f] for r in v)
            return f"{st.median(xs):.{nd}f} ({xs[0]:.{nd}f} to {xs[-1]:.{nd}f})"
        def mean(f, nd):
            xs = [r[f] for r in v if f in r]
            return f"{st.mean(xs):.{nd}f}" if xs else ""
        print(f"| {k} | {len(v)} | {sum(r['n'] for r in v)} | {col('p50', 2)} | {col('p99', 1)} | {col('p999', 1)} | "
              f"{pooled(v, .5):.2f} / {pooled(v, .99):.1f} / {pooled(v, .999):.1f} | {mean('kcycles', 0)} | "
              f"{mean('faults', 1)} | {mean('wakes', 1)} / {mean('thread_ms', 1)} | {mean('tlb', 0)} |")
    rng = random.Random(1)
    ref = {r["round"]: r for r in by.get(a.ref, [])}
    print(f"\nagainst {a.ref}, paired by round: mean difference of run quantiles (95% interval over rounds); "
          "pooled quantile ratio (95% interval over rounds)\n")
    print("| allocator | rounds | p50 ms | p99 ms | p99.9 ms | kcycles | pooled p99 ratio | pooled p99.9 ratio |")
    print("|---|---:|---|---|---|---|---|---|")
    for k in order:
        if k == a.ref:
            continue
        pairs = [(ref[r["round"]], r) for r in by[k] if r["round"] in ref]
        if len(pairs) < 2:
            continue
        cells = []
        for f, nd in (("p50", 3), ("p99", 2), ("p999", 2), ("kcycles", 0)):
            diffs = [y[f] - x[f] for x, y in pairs]
            lo, hi = boot_ci(diffs, st.mean, a.boot, rng)
            cells.append(f"{st.mean(diffs):+.{nd}f} ({lo:+.{nd}f} to {hi:+.{nd}f})")
        for q in (.99, .999):
            def ratio(ps):
                return pooled([y for _, y in ps], q) / pooled([x for x, _ in ps], q)
            lo, hi = boot_ci(pairs, ratio, min(a.boot, 1000), rng)
            cells.append(f"{ratio(pairs):.3f} ({lo:.3f} to {hi:.3f})")
        print(f"| {k} | {len(pairs)} | " + " | ".join(cells) + " |")


main()
