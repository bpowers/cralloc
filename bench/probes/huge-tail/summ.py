#!/usr/bin/env python3
"""Summarizes drift runs made with drift.sh, for docs/research/huge-tail.md.

    python3 -I bench/probes/huge-tail/summ.py FILE... [--series]

Per run: resident heap over live bytes at the end of each high phase (peaks) and each low
phase, as the workload's RSS-based `heap` and as the cgroup's `memory.current` less the
program's bookkeeping; first and last peak and the mean of the last three; cycles per
operation; the run's huge-page counters (the cgroup's faults and collapses, the kernel's
fallbacks, which are machine-wide); and the switch's counters (`.ht`). With --series, every
phase end.
"""
import json
import re
import sys
from pathlib import Path

MiB = 2**20


def load(path):
    samples, result, stats, env = [], None, {}, ""
    for line in Path(path).read_text().splitlines():
        if line.startswith("DRIFT "):
            samples.append(json.loads(line[6:]))
        elif line.startswith("BENCH_RESULT "):
            result = json.loads(line[13:])
        elif line.startswith("# cgbefore ") or line.startswith("# cgafter "):
            tag = line.split()[1][2:]
            stats.setdefault(tag, {}).update({k: int(v) for k, v in re.findall(r"(\w+)=(\d+)", line)})
        elif line.startswith("# before ") or line.startswith("# after "):
            tag = line.split()[1]
            stats.setdefault(tag, {}).update({k: int(v) for k, v in re.findall(r"(\w+)=(\d+)", line)})
        elif line.startswith("# env: "):
            env = line[7:]
    return samples, result, stats, env


def ends(samples):
    """The last load sample of each run of samples with the same phase."""
    out = []
    load = [s for s in samples if s["kind"] == "load"]
    for i, s in enumerate(load):
        if i + 1 == len(load) or load[i + 1]["phase"] != s["phase"]:
            out.append(s)
    return out


def ratio_cg(s):
    return (s["cgroup"] - s["book"]) / s["live"]


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    series = "--series" in sys.argv
    print("| run | peaks rss first/last/last3 | peaks cgroup last3 | lows rss last3 | lows cgroup last3 | quiet rss | thp at peaks MiB (mean of all but first) | cyc/op | free 2 MiB at start | thp faults | collapses | fallbacks (machine) | ht |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for f in args:
        samples, result, stats, env = load(f)
        e = ends(samples)
        hi = [s for s in e if s["phase"].startswith("hi")]
        lo = [s for s in e if s["phase"].startswith("lo")]
        quiet = [s for s in samples if s["kind"] == "quiet"]
        def m3(xs, fn):
            xs = xs[-3:]
            return sum(fn(x) for x in xs) / len(xs) if xs else float("nan")
        cyc = ""
        if result:
            cyc = f"{result['primary']['value']:.0f}"
            lp = result.get("phases", {}).get("load", {})
            extra = [f"pf={lp.get('page_faults', 0) / 1e3:.0f}k"]
            for k in ("walk_4k", "walk_2m", "dtlb_miss"):
                if k in lp:
                    extra.append(f"{k}={lp[k] / result['metrics']['ops']:.2f}/op")
            cyc += " " + " ".join(extra)
        b, a = stats.get("before", {}), stats.get("after", {})
        d = lambda k: a.get(k, 0) - b.get(k, 0)
        ht = ""
        p = Path(f + ".ht")
        if p.exists():
            lines = p.read_text().splitlines()
            if lines:
                kv = dict(re.findall(r"(\w+)=(\d+)", lines[-1]))
                ht = " ".join(f"{k}={kv[k]}" for k in ("unadvise", "readvise", "splits", "tail_pages", "tails") if k in kv)
        name = Path(f).name
        print(
            f"| {name} | {hi[0]['ratio']:.3f}/{hi[-1]['ratio']:.3f}/{m3(hi, lambda s: s['ratio']):.3f} | {m3(hi, ratio_cg):.3f} | "
            f"{m3(lo, lambda s: s['ratio']):.3f} | {m3(lo, ratio_cg):.3f} | {quiet[-1]['ratio'] if quiet else float('nan'):.3f} | {sum(s['thp'] for s in hi[1:]) / max(1, len(hi) - 1) / MiB:.0f} | {cyc} | {b.get('free2m', '?')} | "
            f"{d('cg_thp_fault_alloc') or ('m' + str(d('thp_fault_alloc')))} | {d('cg_thp_collapse_alloc')} | {d('vm_thp_fault_fallback') or d('thp_fault_fallback')} | {ht} |"
            if hi else f"| {name} | (no samples) |"
        )
        if series:
            for s in e:
                print(f"    {s['epoch']:4} {s['phase']:10} rss/live {s['ratio']:.3f} cg/live {ratio_cg(s):.3f} heap {s['heap'] / MiB:.0f} live {s['live'] / MiB:.0f} thp {s['thp'] / MiB:.0f}")


if __name__ == "__main__":
    main()
