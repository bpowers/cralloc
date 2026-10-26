#!/usr/bin/env python3
"""Summarizes runs of the large-slack probe (docs/research/large-slack.md).

    python3 -I summary.py timed DIR...    timed rounds (timed.sh): Mcycles/op and faults per arm
    python3 -I summary.py memory DIR      memory runs (arms.sh): heap/live at each phase's end
    python3 -I summary.py trims FILE...   the switch's counters (.lsr files): trims, sizes, times

A run's ratio at a phase's end is its last DRIFT sample of that phase (heap over live), and the
quiet ratio its last sample of the quiet minute. Ticks are the cycle counter's; TSC_GHZ (default
4.3, the reference host's) converts them to time.
"""
import json
import os
import sys
from collections import defaultdict
from pathlib import Path

GHZ = float(os.environ.get("TSC_GHZ", "4.3"))


def drift(path):
    rows, result = [], None
    for line in Path(path).read_text().splitlines():
        if line.startswith("DRIFT "):
            rows.append(json.loads(line[6:]))
        elif line.startswith("BENCH_RESULT "):
            result = json.loads(line[13:])
    return rows, result


def arm_of(path):
    name = Path(path).name
    return name.split(".", 1)[1].rsplit(".txt", 1)[0]


def timed(dirs):
    by = defaultdict(list)
    for d in dirs:
        for p in sorted(Path(d).glob("*.txt")):
            _, r = drift(p)
            if not r:
                continue
            load = r["phases"]["load"]
            by[arm_of(p)].append((r["primary"]["value"] / 1e6, load["page_faults"]))
    base = None
    for arm in sorted(by):
        if arm in ("lsr-main", "t0a0"):
            base = base or sum(c for c, _ in by[arm]) / len(by[arm])
    print(f"{'arm':10} {'Mcycles/op':>24} {'mean':>6} {'faults (M)':>20}")
    for arm, runs in sorted(by.items()):
        mean = sum(c for c, _ in runs) / len(runs)
        cyc = ", ".join(f"{c:.2f}" for c, _ in runs)
        flt = ", ".join(f"{f / 1e6:.1f}" for _, f in runs)
        print(f"{arm:10} {cyc:>24} {mean:6.2f} {flt:>20}")


def memory(d):
    for p in sorted(Path(d).glob("*.txt")):
        rows, r = drift(p)
        if not rows:
            continue
        load = [x for x in rows if x.get("kind") == "load"]
        quiet = [x for x in rows if x.get("kind") != "load"]
        phases = []
        for x in load:
            if not phases or phases[-1][0] != x["phase"] or phases[-1][1] != x["epoch"] // max(1, len(load) // 16):
                pass
        # The end of each phase of each cycle: the last sample before the phase changes.
        ends = []
        for i, x in enumerate(load):
            if i + 1 == len(load) or load[i + 1]["phase"] != x["phase"]:
                ends.append((x["phase"], x["heap"] / x["live"]))
        last = ends[-4:]
        q = quiet[-1]["heap"] / quiet[-1]["live"] if quiet else None
        cycles = [f"{e:.2f}" for _, e in ends]
        print(f"{arm_of(p):10} last cycle: " + ", ".join(f"{v:.2f}" for _, v in last)
              + (f"  quiet {q:.2f}" if q else "") + f"   every phase end: {' '.join(cycles)}")


def trims(files):
    for f in files:
        for line in Path(f).read_text().splitlines():
            if not line.startswith("lsr "):
                continue
            kv = dict(x.split("=", 1) for x in line.split()[1:])
            n = int(kv["trims"])
            if n == 0:
                print(f"{f}: trim={kv['trim']} age={kv['age']} no trims")
                continue
            pages = [int(x) for x in kv["pages_log2"].split(",")]
            ticks = [int(x) for x in kv["ticks_log2"].split(",")]
            def quant(h, q):
                need, acc = q * sum(h), 0
                for i, c in enumerate(h):
                    acc += c
                    if acc >= need:
                        return (1 << i) if i else 0
                return 1 << (len(h) - 1)
            print(f"{f}: trim={kv['trim']} age={kv['age']} trims={n} pages={int(kv['trim_pages'])} "
                  f"mean {int(kv['trim_pages']) / n:.0f} pages; max {kv['max_pages']} pages; "
                  f"time mean {int(kv['ticks']) / n / GHZ / 1e3:.1f} us, max {int(kv['max_ticks']) / GHZ / 1e3:.0f} us; "
                  f"p50/p99/p99.9 pages < {quant(pages, .5)}/{quant(pages, .99)}/{quant(pages, .999)}, "
                  f"ticks < {quant(ticks, .5)}/{quant(ticks, .99)}/{quant(ticks, .999)}; "
                  + (f"slowest {int(kv['max_ticks']) / GHZ / 1e3:.0f} us over {kv['slowest_pages']} pages, largest {kv['max_pages']} pages in {int(kv['largest_ticks']) / GHZ / 1e3:.0f} us, "
                     f"{kv['big']} of 1024+ pages at {int(kv['big_ticks']) / max(1, int(kv['big'])) / GHZ / 1e3:.0f} us mean; " if 'big' in kv else "")
                  + f"refaulted {kv['refaulted']} of {kv['checked']} pages checked "
                  f"({int(kv['refaulted']) / max(1, int(kv['checked'])):.1%}), "
                  f"tenants {kv['tenants_refaulted']} of {kv['tenants_checked']}")
            print(f"    pages by 2^k: {pages}")
            print(f"    ticks by 2^k: {ticks}")


if __name__ == "__main__":
    cmd, args = sys.argv[1], sys.argv[2:]
    {"timed": timed, "memory": lambda a: memory(a[0]), "trims": trims}[cmd](args)
