#!/usr/bin/env python3
"""Server tails over placement sweeps, new sessions against old: tails.py [--old FILE...] FILE...

Reads benchrun results (jsonl or .gz) whose allocators are `NAME@padNN` (standings/pad-entries.sh)
or plain names. For each job and allocator, over the valid runs:

- cycles a request at the ranked rate (s70): the mean of the pads' medians and their range;
- p50, p99 and p99.9 at s70: the median over runs and the lowest and highest run, and p99.9's
  median at each pad; for the RPC kernel jobs (closed loop, no p99.9), the client's p50 and p99;
- each percentile against the base allocator's: the difference of the medians over runs, and a
  verdict on the pads' medians, as standings/servers.py judges cycles: "resolved" when the two
  ranges over the pads do not overlap, or an exact permutation test of the pads' medians puts
  the means apart at p < 0.05; else "unresolved". "pads n/m" says at how many of the pads both
  ran the base was lower;
- the day's closed-loop segment (`sat`): its rate, the saturated rate of that run;
- the server CPUs' idle setting and entries: deep entries a thousand requests at s70 (median),
  C1 entries a thousand requests, and how many runs had every deep state disabled on every
  server CPU throughout s70.

With --old, the same figures from the old files are printed beside, for jobs in both.
--json OUT writes everything.
"""
import argparse
import gzip
import itertools
import json
import statistics
from collections import defaultdict

PCTS = ("p50", "p99", "p999")


def records(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt") as f:
        for line in f:
            if line.strip():
                yield json.loads(line)


def split(name):
    return tuple(name.split("@", 1)) if "@" in name else (name, "pad00")


def permutation_p(xs, ys):
    """Two-sided p of the difference of means of two sets of pad medians over every split of the
    pooled values into sets of those sizes (as standings/servers.py)."""
    pooled = xs + ys
    n = len(xs)
    obs = abs(statistics.mean(xs) - statistics.mean(ys))
    total = hits = 0
    for idx in itertools.combinations(range(len(pooled)), n):
        s = set(idx)
        a = [pooled[i] for i in idx]
        b = [pooled[i] for i in range(len(pooled)) if i not in s]
        if not b:
            continue
        total += 1
        hits += abs(statistics.mean(a) - statistics.mean(b)) >= obs - 1e-9
    return hits / total if total else 1.0


def run_figures(r):
    """The figures of one valid run: cycles, the percentiles, the sat rate, idle entries."""
    job = r["job"]
    served = r.get("served")
    out = {"cycles": r["primary"]["value"]}
    if not served:
        m = (r.get("driver") or {}).get("metrics") or {}
        out |= {"p50": m.get("p50_us"), "p99": m.get("p99_us"), "p999": None, "kernel": True}
        return out
    g = served["groups"].get("sat" if job.endswith(" sat") else "s70", {})
    lat = g.get("lat_us", {})
    out |= {k: lat.get(k) for k in PCTS}
    sat = served["groups"].get("sat")
    out["sat_rate"] = sat.get("rate") if sat else None
    out["sat_p999"] = sat.get("lat_us", {}).get("p999") if sat else None
    idle = g.get("idle") or {}
    if idle.get("recorded") is True:
        n = lat.get("n") or 0
        out["deep"] = idle.get("deep_per_kreq")
        c1 = sum(s["entries"] for s in idle["states"] if s["name"] == "C1")
        out["c1"] = 1000 * c1 / n if n else None
        cpus = [int(c) for c in idle["cpus"].split(",")]
        deep = [s for s in idle["states"] if s["latency_us"] > idle["deep_latency_us"]]
        out["deep_off"] = all(sorted(s["disabled_on"]) == sorted(cpus) for s in deep) and bool(deep)
    return out


def load(files):
    runs = defaultdict(list)  # (job, alloc) -> [(pad, figures)]
    invalid = defaultdict(int)
    for f in files:
        for r in records(f):
            if r.get("type") != "run":
                continue
            alloc, pad = split(r["allocator"])
            if r.get("valid") is not True:
                invalid[(r["job"], alloc)] += 1
                continue
            runs[(r["job"], alloc)].append((pad, run_figures(r)))
    return runs, invalid


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def summarise(rs):
    per_pad = defaultdict(lambda: defaultdict(list))
    for pad, f in rs:
        for k, v in f.items():
            if isinstance(v, (int, float)) and not isinstance(v, bool):
                per_pad[k][pad].append(v)
    s = {"runs": len(rs)}
    for k in ("cycles",) + PCTS + ("sat_rate", "deep", "c1", "sat_p999"):
        vals = [f.get(k) for _, f in rs if f.get(k) is not None]
        if not vals:
            continue
        pads = {p: statistics.median(v) for p, v in sorted(per_pad[k].items())}
        s[k] = {
            "median": statistics.median(vals),
            "lo": min(vals),
            "hi": max(vals),
            "pads": pads,
            "pad_mean": statistics.mean(pads.values()),
            "pad_lo": min(pads.values()),
            "pad_hi": max(pads.values()),
        }
    offs = [f.get("deep_off") for _, f in rs if "deep_off" in f]
    s["deep_off_runs"] = (sum(1 for x in offs if x), len(offs))
    return s


def compare(b, v, k):
    """v's k against the base's: difference of medians over runs, and the verdict on pads."""
    if k not in b or k not in v:
        return None
    bp, vp = b[k]["pads"], v[k]["pads"]
    pct = 100 * (v[k]["median"] / b[k]["median"] - 1)
    bx, vx = list(bp.values()), list(vp.values())
    apart = b[k]["pad_lo"] > v[k]["pad_hi"] or b[k]["pad_hi"] < v[k]["pad_lo"]
    p = permutation_p(bx, vx) if len(bx) > 1 and len(vx) > 1 else 1.0
    common = [p_ for p_ in bp if p_ in vp]
    lower = sum(1 for p_ in common if bp[p_] < vp[p_])
    return {
        "pct": pct,
        "pad_pct": 100 * (v[k]["pad_mean"] / b[k]["pad_mean"] - 1),
        "resolved": apart or p < 0.05,
        "apart": apart,
        "p": p,
        "base_lower_pads": lower,
        "common_pads": len(common),
    }


def fmt_pct(s, k, nd=1):
    if k not in s:
        return "-"
    x = s[k]
    return f"{x['median']:.{nd}f} ({x['lo']:.{nd}f}-{x['hi']:.{nd}f})"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--old", nargs="*", default=[])
    ap.add_argument("--base", default="asmalloc")
    ap.add_argument("--json")
    a = ap.parse_args()
    new, inv_new = load(a.files)
    old, _ = load(a.old)
    out = {}
    for job in sorted({j for j, _ in new}):
        allocs = sorted({x for j, x in new if j == job}, key=lambda x: (x != a.base, x))
        res = {x: summarise(new[(job, x)]) for x in allocs}
        ores = {x: summarise(old[(job, x)]) for x in allocs if (job, x) in old}
        b = res.get(a.base)
        print(f"\n## {job}")
        print("  allocator          runs  kcyc mean (pad range)   p50 / p99 / p99.9 us, median (lowest-highest run)"
              "                        deep/k  C1/k  deep off  sat req/s")
        for x, s in res.items():
            cyc = s["cycles"]
            line = (f"  {x:18} {s['runs']:4}  {cyc['pad_mean'] / 1e3:7.2f} ({cyc['pad_lo'] / 1e3:.2f}-{cyc['pad_hi'] / 1e3:.2f})"
                    f"  {fmt_pct(s, 'p50'):>20} {fmt_pct(s, 'p99'):>20} {fmt_pct(s, 'p999'):>22}")
            if "deep" in s:
                line += f"  {s['deep']['median']:6.2f} {s['c1']['median']:6.0f}  {s['deep_off_runs'][0]}/{s['deep_off_runs'][1]}"
            if "sat_rate" in s:
                line += f"  {s['sat_rate']['median']:9.0f}"
            if inv_new.get((job, x)):
                line += f"  [{inv_new[(job, x)]} invalid]"
            print(line)
        for k in ("p99", "p999"):
            if b is None or k not in b:
                continue
            print(f"  {k} by pad (medians): " + "  ".join(
                f"{x} " + ",".join(f"{v:.1f}" for v in s[k]["pads"].values()) for x, s in res.items() if k in s))
            for x, s in res.items():
                if x == a.base:
                    continue
                c = compare(b, s, k)
                if c is None:
                    continue
                s.setdefault("vs_base", {})[k] = c
                verdict = ("resolved, ranges apart" if c["apart"] else "resolved") if c["resolved"] else "unresolved"
                print(f"    {k} {x:18} {c['pct']:+6.1f}% against {a.base} (pad means {c['pad_pct']:+.1f}%), {verdict} "
                      f"(p={c['p']:.3f}); {a.base} lower at {c['base_lower_pads']}/{c['common_pads']} pads")
        if ores:
            print("  old (the files given with --old): runs, kcyc mean, p50 / p99 / p99.9 medians, sat req/s")
            for x, s in ores.items():
                line = f"    {x:18} {s['runs']:4}  {s['cycles']['pad_mean'] / 1e3:7.2f}"
                line += "  " + " / ".join(f"{s[k]['median']:.1f}" if k in s else "-" for k in PCTS)
                if "sat_rate" in s:
                    line += f"  {s['sat_rate']['median']:9.0f}"
                if "deep" in s:
                    line += f"  deep/k {s['deep']['median']:.1f}"
                print(line)
                nw = res[x]
                d = [f"cycles {100 * (nw['cycles']['pad_mean'] / s['cycles']['pad_mean'] - 1):+.1f}%"]
                if "sat_rate" in s and "sat_rate" in nw:
                    d.append(f"sat rate {100 * (nw['sat_rate']['median'] / s['sat_rate']['median'] - 1):+.1f}%")
                for k in PCTS:
                    if k in s and k in nw:
                        d.append(f"{k} {nw[k]['median'] - s[k]['median']:+.1f} us")
                print(f"    {'':18} new against old: " + ", ".join(d))
        out[job] = {"new": res, "old": ores}
    if a.json:
        json.dump(out, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
