#!/usr/bin/env python3
"""Summarises the memory ladder (ladder.sh) for docs/research/large-heap-constants.md.

    python3 -I summary.py OUTDIR [--diag] [--json FILE]

For every run OUTDIR/JOB-RUNG.BUILD.txt: resident heap over live at the end of the load (steady:
the mean of the last quarter of samples), at the ends of the high and low phases (lifetime),
after the quiet minute; the seconds after each fall in live (a low phase's start, the quiet
minute's start) until the heap is within a tenth of the fall of where it settles; faults; page
tables and mappings at their most; the background thread's CPU time and wake-ups. With --diag,
for the diagnostic build, the counters of diag_edit.py: each timed kind's count, how many did
work, the longest and the 99.9th percentile, and the walk's laps and passes, growth steps over the
cap, commits.
"""
import json
import re
import sys
from pathlib import Path

TSC_GHZ = 4.3  # the reference host's TSC (kernel: "Refined TSC clocksource calibration: 4300.000 MHz")
KIND_BASE, KIND_SLOTS, HIST = 64, 56, 48
KINDS = ["beat", "hb_purge", "note", "grow_carve", "grow_owed", "grow_lag", "thread_step", "maint_step",
         "round", "lap", "pass", "commit", "dontneed", "advise", "split"]
SCALARS = {"passes": 4, "passes_count_only": 5, "passes_over_cap": 6, "last_ret_small": 7, "last_ret_large": 8,
           "last_returnable": 9, "last_goal": 10, "sum_goal": 11, "carved": 12, "growth_due": 13,
           "growth_due_over_cap": 14, "growth_purged": 15, "laps": 16, "laps_over_interval": 17,
           "last_lap_ticks": 18, "max_retained": 20, "decay_returned_hb": 22, "decay_returned_other": 23,
           "growth_payments_work": 24, "rounds": 25, "last_lap_regions": 26, "passes_ended_round": 27,
           "passes_ended_goal": 28}


def ns(ticks):
    return ticks / TSC_GHZ


def kind_summary(diag, k):
    base = KIND_BASE + k * KIND_SLOTS
    n = diag.get(base, 0)
    if not n:
        return None
    hist = [diag.get(base + 8 + b, 0) for b in range(HIST)]

    def pct(q):
        need, seen = q * n, 0
        for b, c in enumerate(hist):
            seen += c
            if seen >= need:
                return ns(2 ** (b + 1))  # the bucket's upper edge
        return None

    over = lambda us: sum(c for b, c in enumerate(hist) if ns(2 ** b) >= us * 1000)
    return {
        "n": n,
        "work": diag.get(base + 3, 0),
        "total_ms": round(ns(diag.get(base + 1, 0)) / 1e6, 3),
        "max_us": round(ns(diag.get(base + 2, 0)) / 1e3, 1),
        "p999_us_le": round(pct(0.999) / 1e3, 1),
        "over_100us": over(100),
        "over_1ms": over(1000),
        "bytes": diag.get(base + 4, 0),
    }


def load(path):
    head, drift, result = None, [], None
    for line in path.read_text().splitlines():
        if line.startswith("# {"):
            head = json.loads(line[2:])
        elif line.startswith("DRIFT "):
            drift.append(json.loads(line[6:]))
        elif line.startswith("BENCH_RESULT "):
            result = json.loads(line[13:])
    ok = path.read_text().rstrip().splitlines()[-1].startswith("# exit 0")
    watch = []
    wp = Path(str(path) + ".watch.jsonl")
    if wp.exists():
        for line in wp.read_text().splitlines()[1:]:
            r = json.loads(line)
            if r.get("rss_anon") is not None:
                watch.append(r)
    return head, drift, result, watch, ok


def settle_time(samples, key="heap"):
    """Seconds from the first sample until the value is within a tenth of the fall of where it
    ends: (first - last) is the fall; the first time value - last <= 0.1 * fall."""
    if len(samples) < 2:
        return None
    t0, v0 = samples[0]["t_ms"] / 1000, samples[0][key]
    end = samples[-1][key]
    fall = v0 - end
    if fall <= 0:
        return 0.0
    for s in samples:
        if s[key] - end <= 0.1 * fall:
            return round(s["t_ms"] / 1000 - t0, 1)
    return None


def summarize(path, with_diag):
    head, drift, result, watch, ok = load(path)
    m = re.match(r"(\w+)-([\d.]+)\.(.+)\.txt$", path.name)
    job, rung, build = m.group(1), float(m.group(2)), m.group(3)
    out = {"job": job, "rung": rung, "build": build, "ok": ok}
    load_s = [d for d in drift if d["kind"] == "load"]
    quiet = [d for d in drift if d["kind"] == "quiet"]
    if not load_s:
        return out
    out["live_max_gib"] = round(max(d["live"] for d in load_s) / 2**30, 3)
    out["heap_max_gib"] = round(max(d["heap"] for d in load_s) / 2**30, 3)
    if job == "lifetime":
        phases = []
        for d in load_s:
            if phases and phases[-1][0] == d["phase"]:
                phases[-1][1].append(d)
            else:
                phases.append([d["phase"], [d]])
        out["peaks"] = [round(p[1][-1]["ratio"], 3) for p in phases if p[0].startswith("hi")]
        out["troughs"] = [round(p[1][-1]["ratio"], 3) for p in phases if p[0].startswith("lo")]
        out["trough_heap_gib"] = [round(p[1][-1]["heap"] / 2**30, 3) for p in phases if p[0].startswith("lo")]
        # The fall in live happens over a low phase's first operations; time it from the
        # phase's first sample, to the end of the phase.
        out["trough_settle_s"] = [settle_time([prev[1][-1]] + p[1]) for prev, p in zip(phases, phases[1:])
                                  if p[0].startswith("lo")]
        out["phase_s"] = round((load_s[-1]["t_ms"] - load_s[0]["t_ms"]) / 1000 / max(len(phases), 1), 1)
    else:
        q = max(len(load_s) // 4, 1)
        out["ratio_end"] = round(sum(d["ratio"] for d in load_s[-q:]) / q, 3)
        out["ratio_max"] = round(max(d["ratio"] for d in load_s), 3)
    if quiet:
        out["ratio_quiet"] = round(quiet[-1]["ratio"], 3)
        out["quiet_heap_gib"] = round(quiet[-1]["heap"] / 2**30, 3)
        out["quiet_settle_s"] = settle_time([load_s[-1]] + quiet)
        out["quiet_returned_gib"] = round((load_s[-1]["heap"] - quiet[-1]["heap"]) / 2**30, 3)
    if result:
        ph = result["phases"]["load"]
        out["cycles_op"] = round(result["primary"]["value"], 1)
        out["faults_load"] = ph.get("page_faults")
        out["faults_per_kop"] = round(1000 * ph.get("page_faults", 0) / max(result["metrics"]["ops"], 1), 4)
        out["load_s"] = round(ph["wall_ns"] / 1e9, 1)
    if watch:
        out["pte_max_mib"] = round(max(w["pte"] or 0 for w in watch) / 2**20, 1)
        out["maps_max"] = max(w["maps"] for w in watch)
        bg = [w for w in watch if w.get("bg_ns") is not None]
        if bg:
            out["bg_cpu_ms"] = round(bg[-1]["bg_ns"] / 1e6, 1)
            out["bg_wakeups"] = bg[-1]["bg_vol"]
            out["bg_start_s"] = bg[0]["t"]
        gaps = [b["t"] - a["t"] for a, b in zip(watch, watch[1:])]
        out["max_sample_gap_s"] = round(max(gaps), 1) if gaps else None
        if with_diag and "diag" in watch[-1]:
            diag = {int(k): v for k, v in watch[-1]["diag"].items()}
            out["diag"] = {name: kind_summary(diag, k) for k, name in enumerate(KINDS) if kind_summary(diag, k)}
            sc = {name: diag.get(i, 0) for name, i in SCALARS.items()}
            sc["last_lap_ms"] = round(ns(sc.pop("last_lap_ticks")) / 1e6, 1)
            out["scalars"] = sc
    return out


def main():
    d = Path(sys.argv[1])
    with_diag = "--diag" in sys.argv
    rows = [summarize(p, with_diag) for p in sorted(d.glob("*.txt"))]
    rows.sort(key=lambda r: (r["job"], r["rung"], r["build"]))
    if "--json" in sys.argv:
        Path(sys.argv[sys.argv.index("--json") + 1]).write_text(json.dumps(rows, indent=1))
    for r in rows:
        diag = r.pop("diag", None)
        sc = r.pop("scalars", None)
        print(json.dumps(r))
        if diag:
            for k, v in diag.items():
                print("   ", k, json.dumps(v))
            print("   ", json.dumps(sc))


if __name__ == "__main__":
    main()
