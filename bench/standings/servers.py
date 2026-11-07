#!/usr/bin/env python3
"""The servers' standing over placement sweeps: servers.py [--base cralloc] FILE... (jsonl or .gz)

Reads benchrun results whose allocators are `NAME@padNN` (pad-entries.sh) or plain names, and
prints for each job:

- time: each allocator's median server cycles a request at the ranked rate per pad, the mean
  and range of those medians over the pads, the mean against the base's, and whether the
  difference is resolved: the two allocators' ranges over the pads do not overlap ("ranges
  apart"), or they overlap but an exact permutation test of the pads' medians puts the means
  apart at p < 0.05 ("ranges overlap"). Otherwise the difference is inside the range: a choice
  of placement could make or undo it.
- tails at the ranked rate (s70): p50, p99 and p99.9 in us, the median over every run (every
  run is one build at one placement), and the range of p99.9 over the runs; beside them, how
  often the server's CPUs entered deep idle states, per thousand requests (median over the
  runs that recorded it, "not recorded" for older results), and p99.9 against the base's,
  marked "confounded by idle" instead where the two deep-idle rates differ materially (the
  test is benchrun's, `bench/runner/src/cpuidle.rs`, whose constants this reads).
- memory: the resident peak under load and the resident set at the end of the plan (after the
  connections closed, before the server is stopped), medians over the runs, MiB; and the cgroup
  peak.
- for a saturated job (`sat`): the saturated rate and the closed loop's p50, p99 and p99.9.

--json OUT writes the numbers as well.
"""
import argparse
import gzip
import itertools
import json
import re
import statistics
from collections import defaultdict
from pathlib import Path

MiB = 2**20


def runner_const(name):
    """A numeric constant of benchrun's cpuidle.rs, so that this report and benchrun's judge a
    comparison of tails by the same test."""
    src = Path(__file__).resolve().parents[1] / "runner/src/cpuidle.rs"
    m = re.search(rf"^pub const {name}: \w+ = ([0-9.]+);$", src.read_text(), re.M)
    if not m:
        raise SystemExit(f"servers.py: cannot read {name} from {src}")
    return float(m.group(1))


CONFOUND_RATIO = runner_const("CONFOUND_RATIO")
CONFOUND_FLOOR = runner_const("CONFOUND_FLOOR")


def confounded(a, b):
    """Whether two deep-idle rates differ enough that a difference in tails between them may be
    the CPUs' exits from deep idle and not the allocators (cpuidle.rs, `confounded`)."""
    lo, hi = sorted((a, b))
    return hi - lo >= CONFOUND_FLOOR and hi >= lo * CONFOUND_RATIO


def deep_per_kreq(group):
    idle = group.get("idle") or {}
    return idle.get("deep_per_kreq") if idle.get("recorded") is True else None


def records(path):
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt") as f:
        for line in f:
            if line.strip():
                yield json.loads(line)


def split(name):
    return tuple(name.split("@", 1)) if "@" in name else (name, "pad00")


def at_end(served, col):
    """The curve's column in MiB at the end of the plan. The curve goes on past it into the
    server's shutdown, whose frees would count as memory it gave back while idle."""
    end = next((m[0] for m in served.get("marks_ms") or [] if m[1] == "end"), float("inf"))
    c = [p for p in served.get("curve_kib") or [] if p[0] <= end]
    return c[-1][col] / 1024 if c else None


def permutation_p(xs, ys):
    """Two-sided p of the difference of means between two sets of pad medians, by every way of
    splitting the pooled values into sets of those sizes. Each pad's median is one draw of
    placement; the test asks whether the two allocators' draws come from one distribution."""
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--base", default="cralloc")
    ap.add_argument("--json")
    a = ap.parse_args()
    runs = defaultdict(list)  # (job, alloc) -> runs
    invalid = defaultdict(int)
    for f in a.files:
        for r in records(f):
            if r.get("type") != "run":
                continue
            alloc, pad = split(r["allocator"])
            if r.get("valid") is not True:
                invalid[(r["job"], alloc)] += 1
                continue
            r["_pad"] = pad
            runs[(r["job"], alloc)].append(r)
    out = {}
    for job in sorted({j for j, _ in runs}):
        allocs = sorted({x for j, x in runs if j == job}, key=lambda x: (x != a.base, x))
        print(f"\n## {job}")
        sat = job.endswith(" sat")
        anon = "meilisearch" in job
        # A kernel job (rpc log c64) has no served record: time only.
        served = all(r.get("served") for x in allocs for r in runs[(job, x)])
        res = {}
        for x in allocs:
            rs = runs[(job, x)]
            per = defaultdict(list)
            for r in rs:
                per[r["_pad"]].append(r["primary"]["value"])
            meds = {p: statistics.median(v) for p, v in sorted(per.items())}
            vals = list(meds.values())
            res[x] = {
                "runs": len(rs),
                "pads": {p: [m, len(per[p])] for p, m in meds.items()},
                "mean": sum(vals) / len(vals),
                "lo": min(vals),
                "hi": max(vals),
                "invalid": invalid.get((job, x), 0),
            }
            if not served:
                continue
            lat = defaultdict(list)
            p999_pad = defaultdict(list)
            deep = []
            for r in rs:
                g = r["served"]["groups"].get("sat" if sat else "s70", {})
                for k in ("p50", "p99", "p999"):
                    lat[k].append(g.get("lat_us", {}).get(k))
                p999_pad[r["_pad"]].append(g.get("lat_us", {}).get("p999"))
                d = deep_per_kreq(g)
                if d is not None:
                    deep.append(d)
            res[x] |= {
                "deep_per_kreq": statistics.median(deep) if deep else None,
                "deep_recorded": len(deep),
                "p999_pads": dict(sorted(p999_pad.items())),
                "p50": statistics.median(lat["p50"]),
                "p99": statistics.median(lat["p99"]),
                "p999": statistics.median(lat["p999"]),
                "p999_lo": min(lat["p999"]),
                "p999_hi": max(lat["p999"]),
                "loaded_peak": statistics.median(
                    [r["served"]["rss_max_loaded"] / MiB for r in rs if r["served"].get("rss_max_loaded")] or [0]
                ),
                "end": statistics.median([at_end(r["served"], 2 if anon else 1) or 0 for r in rs]),
                "cgroup_peak": statistics.median([(r.get("cgroup") or {}).get("peak", 0) / MiB for r in rs]),
            }
            if sat:
                res[x]["rate"] = statistics.median([r["served"]["groups"]["sat"]["rate"] for r in rs])
        out[job] = res
        b = res.get(a.base)
        metric = "sat req/s" if sat else ("k cycles/req" if served else "the job's metric / 1000")
        print(f"\ntime ({metric} for sat jobs the rate), per pad medians (runs), mean (range over pads), against {a.base}")
        pads = sorted({p for x in res.values() for p in x["pads"]})
        for x, v in res.items():
            cells = " ".join(f"{v['pads'][p][0] / 1e3:7.2f}({v['pads'][p][1]})" if p in v["pads"] else "      -   " for p in pads)
            line = f"  {x:18} {cells}  mean {v['mean'] / 1e3:7.2f} ({v['lo'] / 1e3:.2f} to {v['hi'] / 1e3:.2f}, {100 * (v['hi'] / v['lo'] - 1):.1f}%)"
            if b and x != a.base:
                d = 100 * (v["mean"] / b["mean"] - 1)
                apart = v["lo"] > b["hi"] or v["hi"] < b["lo"]
                p = permutation_p([m for m, _ in b["pads"].values()], [m for m, _ in v["pads"].values()])
                v["vs_base"] = {"pct": d, "apart": apart, "p": p}
                verdict = "resolved, ranges apart" if apart else ("resolved, ranges overlap" if p < 0.05 else "inside the range")
                line += f"  {d:+.1f}% {verdict} (p={p:.3f})"
            if v["invalid"]:
                line += f"  [{v['invalid']} invalid]"
            print(line)
        if not served:
            continue
        if sat:
            print("  saturated rate, req/s: " + ", ".join(f"{x} {v['rate']:.0f}" for x, v in res.items()))
        print(
            f"\ntails at {'saturation (closed loop)' if sat else 's70'}, us: p50 / p99 / p99.9 (p99.9 range over runs); "
            f"deep idle entries of the server's CPUs per thousand requests; p99.9 against {a.base}"
        )
        flagged = False
        for x, v in res.items():
            d = v["deep_per_kreq"]
            if d is None:
                deep_cell = "deep not recorded"
            elif v["deep_recorded"] < v["runs"]:
                deep_cell = f"deep {d:6.1f} ({v['deep_recorded']} of {v['runs']} runs)"
            else:
                deep_cell = f"deep {d:6.1f}"
            vs = ""
            if b and x != a.base and b.get("p999") is not None:
                bd = b["deep_per_kreq"]
                diff = f"{v['p999'] - b['p999']:+.1f} us"
                if d is None or bd is None:
                    vs = f"  {diff}, idle not recorded"
                elif confounded(d, bd):
                    vs = "  confounded by idle"
                    flagged = True
                else:
                    vs = f"  {diff}"
                v["p999_vs_base"] = vs.strip()
            print(f"  {x:18} {v['p50']:8.1f} {v['p99']:8.1f} {v['p999']:8.1f} ({v['p999_lo']:.1f} to {v['p999_hi']:.1f})  {deep_cell}{vs}")
            print(f"  {'':18} p99.9 by pad: " + "; ".join(f"{p} " + ",".join(f"{t:.0f}" for t in ts) for p, ts in v["p999_pads"].items()))
        if flagged:
            print(
                f"  confounded by idle: the deep-idle rates differ by a factor of {CONFOUND_RATIO:g} or more (and by at "
                f"least {CONFOUND_FLOOR:g} a thousand requests), so the p99.9 differ in how often the server's CPUs woke "
                "from deep idle, which follows the server's speed (docs/research/measuring.md, "
                "\"A tail at one fixed rate can rank the CPU's idle states\")"
            )
        if not sat:
            print(f"\nmemory, MiB ({'anonymous' if anon else 'resident'} at the end): loaded peak, at the end, cgroup peak")
            for x, v in res.items():
                print(f"  {x:18} {v['loaded_peak']:8.1f} {v['end']:8.1f} {v['cgroup_peak']:8.1f}")
    if a.json:
        json.dump(out, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
