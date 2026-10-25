#!/usr/bin/env python3
"""Summarizes the mini-redis runs of docs/research/heap-drift.md (redis.py, redis-matrix.sh).

    python3 -I bench/probes/heap-drift/redis_summary.py OUTDIR

For each build, at the end of every phase and at the end of the quiet minute: the server's
anonymous resident memory, and its cgroup's memory.current, over the live bytes the live-bytes
build's run held at the same phase and second. The program's live bytes do not depend on the
allocator, so one run of that build is every build's denominator.
"""
import json
import sys
from pathlib import Path

MiB = 2**20


def load(path):
    rows = [json.loads(l) for l in Path(path).read_text().splitlines()]
    return rows[0], [r for r in rows if "t" in r], rows[-1]


def ends(samples):
    """The last sample of each phase index, in order."""
    out = {}
    for s in samples:
        out[s["i"]] = s
    return [out[i] for i in sorted(out)]


def live_at(live, i, t):
    """The live-bytes run's live bytes at phase `i`, second `t` of it (the nearest sample)."""
    cands = [s for s in live if s["i"] == i and s["live"] is not None]
    if not cands:
        return None
    return min(cands, key=lambda s: abs(s["t_phase"] - t))["live"]


def main():
    out = Path(sys.argv[1])
    _, live, _ = load(out / "redis.live-bytes.jsonl")
    files = sorted(out.glob("redis.*.jsonl"))
    header, _, _ = load(files[0])
    names = ["preload"] + header["phases"] + ["quiet"]
    print("Resident anonymous memory over live bytes at the end of each phase (cgroup memory.current over live in brackets)\n")
    print("| build | " + " | ".join(names) + " |")
    print("|---|" + "---|" * len(names))
    lives = [live_at(live, s["i"], s["t_phase"]) for s in ends(live)]
    print("| live MiB | " + " | ".join(f"{l / MiB:.0f}" if l else "-" for l in lives) + " |")
    for f in files:
        build = f.name.split(".")[1]
        if build == "live-bytes":
            continue
        _, samples, last = load(f)
        if not last.get("ok"):
            print(f"| {build} | run not clean |")
            continue
        cells = []
        for s in ends(samples):
            l = live_at(live, s["i"], s["t_phase"])
            if not l or s["rss_anon"] is None:
                cells.append("-")
                continue
            cg = f" ({s['cgroup'] / l:.2f})" if s.get("cgroup") else ""
            cells.append(f"{s['rss_anon'] / l:.2f}{cg}")
        print(f"| {build} | " + " | ".join(cells) + " |")


if __name__ == "__main__":
    main()
