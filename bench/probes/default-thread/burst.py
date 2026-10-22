#!/usr/bin/env python3
"""The burst after the silence, run by run, from sessions of session.sh with BURST_AT on the
scratch diagnostic builds (diag_edit.py): benchrun's results, the watcher's samples (every
250 ms, and every millisecond around the burst) and every request's record.

  burst.py DIR [TAG...]

For each run: the silence (what was returned, by which rule and class, how many drains, and
what the clocked pass's last look at the floor read), the burst's first 5 ms and 5 to 50 ms
(requests, latency, page faults, pages claimed clean by class, slow paths, claims by class),
and the slowest requests of the first 5 ms with the faults around them.
"""
import json
import struct
import sys
from collections import defaultdict
from pathlib import Path

CLEANED, RETURNED, CLAIMS = 0, 128, 256
SLOW_ALLOC, SLOW_FREE = 384, 385
DECAY_IDLE, DECAY_ANY, DECAY_MAINT, GROWTH, HOLD, HOLD_MAINT = 390, 391, 392, 393, 394, 395
PASS_LOOKS, PASS_AT_FLOOR, RET_SMALL, RET_LARGE, FLOOR, GOAL, GOAL_SUM = 400, 401, 402, 403, 404, 405, 406
DRAINS, SHEDS = 410, 411
MiB = 2 ** 20


def class_sizes():
    """Class index to slot size, from the allocator's own table (classes/mod.rs is the
    authority; this reads it through a tiny probe the first time)."""
    p = Path(__file__).with_name("classes.json")
    return {int(k): v for k, v in json.loads(p.read_text()).items()} if p.exists() else {}


SIZES = class_sizes()


def size_of(i):
    s = SIZES.get(i)
    if s is None:
        return f"#{i}"
    return f"{s // 1024} KiB" if s >= 1024 and s % 1024 == 0 else f"{s} B"


def g(d, i):
    return d.get(str(i), d.get(i, 0)) if d else 0


def delta(a, b, i):
    return g(b, i) - g(a, i)


def by_class(a, b, base):
    out = {}
    for i in range(112):
        v = delta(a, b, base + i)
        if v:
            out[i] = v
    return out


def fmt_classes(m, scale=1, unit=""):
    top = sorted(m.items(), key=lambda kv: -kv[1])[:8]
    return ", ".join(f"{size_of(i)}: {v / scale:.0f}{unit}" for i, v in top)


def main():
    d = Path(sys.argv[1])
    tags = sys.argv[2:] or sorted({f.name.split(".")[0] for f in d.glob("*.jsonl")})
    for tag in tags:
        runs = [json.loads(l) for l in open(d / f"{tag}.jsonl") if '"type":"run"' in l]
        watch = [json.loads(l) for l in open(d / f"{tag}.watch.jsonl")]
        procs = [w for w in watch if w["type"] == "proc"]
        for r in runs:
            exe = r["argv"][0]
            env = r.get("env") or {}
            cand = [p for p in procs if p["exe"].endswith(exe.split("/bench/")[-1]) and abs(p["start"] - r["started"]) < 3]
            if not cand:
                print(f"{tag} {r['allocator']}: no watcher record")
                continue
            pid = cand[0]["pid"]
            gen = r["served"]["generator"]
            m0 = gen["start_mono_ns"]
            segs = {s["name"]: s for s in gen["segments"]}
            t = lambda name: m0 + int(segs[name]["begin_ms"] * 1e6)
            off0 = m0 + int([s for s in gen["segments"] if s["name"] == "off"][0]["begin_ms"] * 1e6)
            s70 = [s for s in gen["segments"] if s["name"] == "s70"]
            s70_end = m0 + int(s70[-1]["end_ms"] * 1e6)
            b0 = t("b5")
            slow = [w for w in watch if w["type"] == "s" and w["pid"] == pid and "m" in w]
            fast = [w for w in watch if w["type"] == "f" and w["pid"] == pid]
            at = lambda ts, m: max((w for w in ts if w["m"] <= m), key=lambda w: w["m"], default=None)
            print(f"\n=== {tag} {r['allocator']} (pid {pid})")
            # The silence: from the end of the load to the burst.
            a, b = at(slow, s70_end), at(slow, b0)
            if a and b and a.get("diag") is not None:
                da, db = a["diag"], b["diag"]
                print(f"  silence: returned {sum(by_class(da, db, RETURNED).values()) / MiB:.1f} MiB: "
                      f"clocked pass {(delta(da, db, DECAY_IDLE) + delta(da, db, DECAY_ANY)) / MiB:.1f} "
                      f"(runs of any length {delta(da, db, DECAY_ANY) / MiB:.1f}, by maintain {delta(da, db, DECAY_MAINT) / MiB:.1f}), "
                      f"hold {delta(da, db, HOLD) / MiB:.1f} (by maintain {delta(da, db, HOLD_MAINT) / MiB:.1f}), "
                      f"growth {delta(da, db, GROWTH) / MiB:.1f}")
                print(f"    by class: {fmt_classes(by_class(da, db, RETURNED), MiB, ' MiB')}")
                print(f"    drains {delta(da, db, DRAINS)}, sheds {delta(da, db, SHEDS)}; pass looks {delta(da, db, PASS_LOOKS)}, "
                      f"at the floor {delta(da, db, PASS_AT_FLOOR)}; goals set {delta(da, db, GOAL_SUM) / MiB:.1f} MiB")
                for name, w in (("end of load", da), ("before burst", db)):
                    print(f"    last pass look ({name}): retained small {g(w, RET_SMALL) / MiB:.1f} MiB, large "
                          f"{g(w, RET_LARGE) / MiB:.1f} MiB, floor {g(w, FLOOR) / MiB:.0f} MiB, goal {g(w, GOAL) / MiB:.1f} MiB")
                print(f"    faults in the silence {b['minflt'] - a['minflt']}")
            # The burst, millisecond by millisecond.
            if fast and fast[0].get("diag") is not None:
                base = at(fast, b0)
                for lo, hi in ((0, 5), (5, 50)):
                    x, y = at(fast, b0 + lo * 1_000_000), at(fast, b0 + hi * 1_000_000)
                    if not (x and y):
                        continue
                    dx, dy = x["diag"], y["diag"]
                    cl = by_class(dx, dy, CLEANED)
                    print(f"  burst {lo}-{hi} ms: faults {y['minflt'] - x['minflt']}, pages claimed clean {sum(cl.values())} "
                          f"({fmt_classes(cl)}), slow paths {delta(dx, dy, SLOW_ALLOC) + delta(dx, dy, SLOW_FREE)}, "
                          f"claims {sum(by_class(dx, dy, CLAIMS).values())}, returned {sum(by_class(dx, dy, RETURNED).values()) / 1024:.0f} KiB "
                          f"(pass {(delta(dx, dy, DECAY_IDLE) + delta(dx, dy, DECAY_ANY)) / 1024:.0f}, by maintain {delta(dx, dy, DECAY_MAINT) / 1024:.0f}, "
                          f"hold {delta(dx, dy, HOLD) / 1024:.0f}, growth {delta(dx, dy, GROWTH) / 1024:.0f}), drains {delta(dx, dy, DRAINS)}, "
                          f"pass looks {delta(dx, dy, PASS_LOOKS)}")
                line = []
                for ms in range(-1, 8):
                    x, y = at(fast, b0 + ms * 1_000_000), at(fast, b0 + (ms + 1) * 1_000_000)
                    if x and y:
                        line.append(f"{ms}:{y['minflt'] - x['minflt']}f/{sum(by_class(x['diag'], y['diag'], CLEANED).values())}c")
                print("  per ms (faults/pages claimed clean):", " ".join(line))
            elif fast:
                x, y = at(fast, b0), at(fast, b0 + 5_000_000)
                if x and y:
                    print(f"  burst 0-5 ms: faults {y['minflt'] - x['minflt']}")
            # The requests of the first 5 ms.
            raw = d / f"{tag}.raw" / f"{r.get('pid', pid)}.raw"
            if not raw.exists():
                raw = d / f"{tag}.raw" / f"{pid}.raw"
            if raw.exists():
                idx = [i for i, s in enumerate(gen["segments"]) if s["name"] in ("b5", "b50")]
                bstart = gen["segments"][idx[0]]["begin_ms"] * 1e6
                reqs = []
                with open(raw, "rb") as f:
                    data = f.read()
                for due, lat, seg, fl in struct.iter_unpack("<QQHH", data):
                    if seg in idx:
                        reqs.append(((due - bstart) / 1e3, lat / 1e3, seg))
                first = [q for q in reqs if q[0] < 5000]
                worst = sorted(first, key=lambda q: -q[1])[:5]
                print(f"  first 5 ms: {len(first)} requests; slowest (due us, latency us, done us): "
                      + ", ".join(f"({q[0]:.0f}, {q[1]:.0f}, {q[0] + q[1]:.0f})" for q in worst))
                steady = sorted(q[1] for q in reqs if q[2] == idx[-1])
                p50 = steady[len(steady) // 2] if steady else 0
                excess = sum(max(0.0, q[1] - p50) for q in first) / 1e3
                print(f"    excess over the 5-50 ms median ({p50:.0f} us): {excess:.2f} request-ms")
                over = [q for q in first if q[1] > 300]
                if over:
                    print(f"    over 300 us: {len(over)}, due {min(q[0] for q in over):.0f} to {max(q[0] for q in over):.0f} us")


if __name__ == "__main__":
    main()
