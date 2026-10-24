#!/usr/bin/env python3
"""The account of resident memory under load, for docs/research/meili-peak.md, from a session's
results (session.sh): benchrun's records, the watcher's, and the samplers' files.

  account.py DIR TAG [--window s70] [--at peak|mean] [--diag]

For each run: the window is the plan's segments named WINDOW (all of them, first to last).
asmalloc's sampling build (memstat_peak.rs) and jemalloc's (je_peak.rs) are read at the sample
with the most anonymous memory in the window (`--at peak`) or averaged over it (`--at mean`),
and broken down by category in MiB. With --diag, asmalloc's counters over the window: what
each rule returned, what large claims faulted back and why, growth carved, in place and paid.
Every run prints the cgroup's anonymous memory over the window from the watcher, and the
server's page faults a request from benchrun.
"""
import json
import re
import sys
from pathlib import Path

MiB = 2 ** 20
SMALL_MAX = 32 << 10
KV = re.compile(r"(\w+)=(\S+)")
RULES = ["none", "hold", "pass", "growth", "trim", "self"]
AGES = ["<64ms", "64-125ms", "125-250ms", "0.25-0.5s", "0.5-1s", "1-2s", "2-8s", ">8s"]
D = dict(RET_HOLD=0, RET_PASS=1, RET_GROWTH=2, RET_TRIM=3, RET_FREE_ADVISED=4, RET_MOVE=5, RET_SHRINK=6,
         CARVE_SMALL=7, CARVE_LARGE=8, COVER=9, DEBT_TAKEN=10, DEBT_OWED=11, DEBT_GOAL=12, DEBT_CARRIED=13,
         DEBT_DRY=14, DEBT_UNDER_CAP=15, LARGE_CLAIMS=16, LARGE_CLAIM_BYTES=17, LARGE_CLAIM_CLEAN=18,
         LARGE_CLAIM_NONRES=19, LARGE_FREES=20, LARGE_FREE_CHARGE=21, LARGE_FREE_RES=22, SMALL_REVIVE=23,
         RETAINED_SMALL=24, RETAINED_LARGE=25, HOLD_LOOKS=26, HOLD_FINE=27, DEBT_NOW=28, LARGE_CARVES=29,
         KEPT=30, KEPT_REFUSED=31, MOVES=32, MOVE_BYTES=33, PAYMENTS=34, PAY_UNDER_CAP=35)
LARGE_FAULT_BY, SMALL_REVIVE_BY = 64, 112
CLASS = dict(claim=128, fault=256, carve=384, free_charge=512, free_res=640, ret=768, small_carve=896,
             small_revive=1024)


def memstat(path):
    """asmalloc's samples: [{t_ms, ..., 'classes': {size: {...}}, 'diag': {i: v}}]."""
    out, cur = [], None
    for line in open(path, errors="replace"):
        tag = line.split(" ", 1)[0]
        if tag == "at":
            cur = {k: int(v) for k, v in KV.findall(line)}
            cur["classes"] = {}
            out.append(cur)
        elif tag == "atc" and cur is not None:
            kv = {k: int(v) for k, v in KV.findall(line)}
            cur["classes"][kv["size"]] = kv
        elif tag == "atd" and cur is not None:
            cur["diag"] = {int(i): int(v) for i, v in re.findall(r" (\d+)=(\d+)", line)}
    return [s for s in out if "diag" in s]


def jestat(path):
    out = []
    for line in open(path, errors="replace"):
        tag = line.split(" ", 1)[0]
        if tag == "je":
            out.append({k: int(v) for k, v in KV.findall(line)})
        elif tag in ("jeb", "jel", "jex") and out:
            out[-1][tag] = [tuple(int(x) for x in f.split(":")) for f in line.split()[2:]]
    return out


def window(run, name):
    segs = run["served"]["generator"]["segments"]
    w = [s for s in segs if s["name"] == name]
    return w[0]["begin_ms"], w[-1]["end_ms"]


def mb(x):
    return f"{x / MiB:7.1f}"


def asm_categories(s):
    small = [c for size, c in s["classes"].items() if size <= SMALL_MAX]
    large = [c for size, c in s["classes"].items() if size > SMALL_MAX]
    tot = lambda cs, k: sum(c[k] for c in cs)
    cat = {}
    cat["anon (process)"] = s["anon"] - s["self"]
    cat["sampler's own buffers (excluded)"] = s["self"]
    heap = s["heap"]
    cat["asmalloc's reservation, resident"] = heap
    cat["  outside it (glibc arenas, stacks, binary's data)"] = cat["anon (process)"] - heap
    cat["  glibc mallinfo2: in use / free / mmapped"] = (s["g_used"], s["g_free"], s["g_mmap"])
    cat["live: asked for, small classes"] = tot(small, "req_slot")
    cat["live: rounding to the class, small"] = tot(small, "live") - tot(small, "req_slot")
    cat["live: asked for, large classes"] = tot(large, "req_slot")
    cat["live: resident beyond asked, large"] = tot(large, "live") - tot(large, "req_slot")
    cat["free in bitmaps, small: on wholly free pages"] = tot(small, "free_whole")
    cat["free in bitmaps, small: pinned by live neighbours"] = tot(small, "free_res") - tot(small, "free_whole")
    cat["free in bitmaps, large: within the hold"] = s["flyoung"]
    cat["free in bitmaps, large: past the hold"] = s["flold"]
    cat["free in bitmaps, large: no stamp"] = s["flnone"]
    cat["parked: loaded / spare / depot"] = (s["loaded"], s["spare"], s["depot"])
    cat["above watermarks (huge-page tails)"] = s["above"]
    cat["metadata, magazines, slabs, runs"] = s["meta"] + s["mags"] + s["slabs"] + s["runs"]
    cat["retained count (small+large)"] = s["retained"]
    return cat


def je_categories(s):
    cat = {}
    cat["anon (process)"] = s["anon"]
    cat["jemalloc resident"] = s["resident"]
    cat["  outside it (glibc arenas, stacks, binary's data)"] = s["anon"] - s["resident"]
    cat["  glibc mallinfo2: in use / free / mmapped"] = (s["g_used"], s["g_free"], s["g_mmap"])
    cat["live: asked for"] = s["req"]
    cat["live: rounding to the class (allocated - asked)"] = s["allocated"] - s["req"]
    cat["active - allocated (slab space, large page rounding)"] = s["active"] - s["allocated"]
    cat["dirty"] = s["pdirty"]
    cat["muzzy"] = s["pmuzzy"]
    cat["metadata"] = s["metadata"]
    cat["resident - (active + dirty + metadata)"] = s["resident"] - s["active"] - s["pdirty"] - s["metadata"]
    cat["allocated: small / large"] = (s["small"], s["large"])
    return cat


def show(cat):
    for k, v in cat.items():
        if isinstance(v, tuple):
            print(f"  {k:55s} " + " / ".join(mb(x).strip() for x in v))
        else:
            print(f"  {k:55s} {mb(v)}")


def pick(samples, lo, hi, key, how):
    inside = [s for s in samples if lo <= s["t_ms"] <= hi]
    if not inside:
        return None, []
    if how == "peak":
        return max(inside, key=lambda s: s[key]), inside
    return None, inside


def mean_cat(cats):
    out = {}
    for k in cats[0]:
        v = [c[k] for c in cats]
        if isinstance(v[0], tuple):
            out[k] = tuple(sum(x[i] for x in v) / len(v) for i in range(len(v[0])))
        else:
            out[k] = sum(v) / len(v)
    return out


def diag_report(first, last, t):
    d = {i: last["diag"].get(i, 0) - first["diag"].get(i, 0) for i in set(first["diag"]) | set(last["diag"])}
    g = lambda name: d.get(D[name], 0)
    secs = t / 1e3
    print(f"  counters over {secs:.1f} s, MiB a second:")
    for name in ["RET_HOLD", "RET_PASS", "RET_GROWTH", "RET_FREE_ADVISED", "RET_MOVE", "RET_SHRINK", "RET_TRIM",
                 "CARVE_SMALL", "CARVE_LARGE", "COVER", "SMALL_REVIVE", "LARGE_CLAIM_BYTES", "LARGE_CLAIM_CLEAN",
                 "LARGE_CLAIM_NONRES", "LARGE_FREE_CHARGE", "LARGE_FREE_RES", "DEBT_TAKEN", "DEBT_OWED",
                 "DEBT_GOAL", "DEBT_CARRIED", "DEBT_DRY", "DEBT_UNDER_CAP", "KEPT", "KEPT_REFUSED", "MOVE_BYTES"]:
        print(f"    {name:20s} {g(name) / MiB / secs:9.1f}")
    for name in ["LARGE_CLAIMS", "LARGE_FREES", "LARGE_CARVES", "MOVES", "PAYMENTS", "PAY_UNDER_CAP", "HOLD_LOOKS"]:
        print(f"    {name:20s} {g(name) / secs:9.1f} /s")
    print("  large claims' faults by the rule that last purged the region, and how long before, MiB/s:")
    print("    " + " " * 8 + "".join(f"{a:>11s}" for a in AGES))
    for r, rn in enumerate(RULES):
        row = [d.get(LARGE_FAULT_BY + r * 8 + a, 0) for a in range(8)]
        if any(row):
            print(f"    {rn:8s}" + "".join(f"{x / MiB / secs:11.2f}" for x in row))
    print("    (none, first column: pages the last tenant was given and never wrote; second: never purged)")
    rows = []
    for size in sorted(first["classes"].keys() | last["classes"].keys()):
        pass
    print("  by class (large), MiB/s: claimed / faulted / carved / freed charged / freed resident / returned")
    import itertools
    sizes = class_sizes(last)
    for ci, size in sizes.items():
        vals = [d.get(CLASS[k] + ci, 0) for k in ("claim", "fault", "carve", "free_charge", "free_res", "ret")]
        if size > SMALL_MAX and sum(vals) > 0.5 * MiB * secs:
            print(f"    {size:>10d} " + " ".join(f"{v / MiB / secs:8.1f}" for v in vals))
    small_ret = sum(d.get(CLASS["ret"] + ci, 0) for ci, s in sizes.items() if s <= SMALL_MAX)
    small_rev = sum(d.get(CLASS["small_revive"] + ci, 0) for ci, s in sizes.items() if s <= SMALL_MAX)
    print(f"  small classes: returned {small_ret / MiB / secs:.1f} MiB/s, revived {small_rev / MiB / secs:.1f} MiB/s")


def class_sizes(sample):
    """Class index to size: the classes are those of asmalloc::classes, indexed from 1; the sampler
    prints sizes only, so the index is recovered from the sorted list of every class it printed
    plus the known size rule."""
    sizes = []
    s = 8
    while s <= 64:
        sizes.append(s)
        s += 8
    base = 64
    while len(sizes) < 112:
        step = base // 4
        for k in range(1, 5):
            sizes.append(base + k * step)
            if len(sizes) == 112:
                break
        base *= 2
    return {i + 1: size for i, size in enumerate(sizes)}


def main():
    d, tag = Path(sys.argv[1]), sys.argv[2]
    args = sys.argv[3:]
    wname = args[args.index("--window") + 1] if "--window" in args else "s70"
    how = args[args.index("--at") + 1] if "--at" in args else "peak"
    runs = [json.loads(l) for l in open(d / f"{tag}.jsonl") if '"type":"run"' in l]
    watch = [json.loads(l) for l in open(d / f"{tag}.watch.jsonl")]
    procs = [w for w in watch if w["type"] == "proc"]
    gens = [w for w in watch if w["type"] == "gen"]
    for r in runs:
        p = [p for p in procs if abs(p["start"] - r["started"]) < 5]
        if not p or not r.get("served"):
            print(f"\n=== {r['allocator']}: no process or no plan (valid={r.get('valid')})")
            continue
        pid, start = p[0]["pid"], p[0]["start"]
        gen = min(g["start"] for g in gens if g["start"] >= start)
        lo, hi = window(r, wname)
        plan0 = (gen - start) * 1e3
        samples = [w for w in watch if w["type"] == "s" and w.get("pid") == pid]
        cg = [s["cg_anon"] for s in samples if s.get("cg_anon") and lo <= (s["t"] - gen) * 1e3 <= hi]
        g = r["served"]["groups"].get(wname, {})
        server = g.get("server", {})
        lat = g.get("lat_us", {})
        print(f"\n=== {tag} {r['allocator']} pid {pid} valid={r.get('valid')} window {wname} {lo / 1e3:.0f}-{hi / 1e3:.0f} s")
        print(f"  faults/req {server.get('faults_per_req') or 0:.1f}  Mcycles/req {(server.get('cycles_per_req') or 0) / 1e6:.2f}"
              f"  p50/p99/p99.9 ms {lat.get('p50', 0) / 1e3:.2f} / {lat.get('p99', 0) / 1e3:.1f} / {lat.get('p999', 0) / 1e3:.1f}")
        if cg:
            print(f"  watcher cgroup anon: mean {mb(sum(cg) / len(cg))} peak {mb(max(cg))}")
        ms = list((d / "memstat").glob(f"*.{pid}"))
        js = list((d / "jestat").glob(f"*.{pid}"))
        if ms:
            samp = memstat(ms[0])
            for s in samp:
                s["t_ms"] -= plan0
            top, inside = pick(samp, lo, hi, "anon", how)
            if not inside:
                continue
            if how == "peak":
                print(f"  at the sample with most anonymous memory, {top['t_ms'] / 1e3:.2f} s into the plan:")
                show(asm_categories(top))
            else:
                print(f"  mean over {len(inside)} samples:")
                show(mean_cat([asm_categories(s) for s in inside]))
            if "--diag" in args:
                diag_report(inside[0], inside[-1], inside[-1]["t_ms"] - inside[0]["t_ms"])
        if js:
            samp = jestat(js[0])
            for s in samp:
                s["t_ms"] -= plan0
            top, inside = pick(samp, lo, hi, "anon", how)
            if not inside:
                continue
            if how == "peak":
                print(f"  at the sample with most anonymous memory, {top['t_ms'] / 1e3:.2f} s into the plan:")
                show(je_categories(top))
            else:
                print(f"  mean over {len(inside)} samples:")
                show(mean_cat([je_categories(s) for s in inside]))


if __name__ == "__main__":
    main()
