#!/usr/bin/env python3
"""raw.py RESULTS.jsonl RAWDIR [--slow US] [--segs s70,t00,...]: every request of each run, from
the load generator's records (`BENCH_RAW_DIR`, 20 bytes a request: due ns, latency ns, segment,
flags), set beside benchrun's record of the run.

For each run: per named segment the p99, p99.9 and the share of requests slower than --slow;
and over the 70% segments together, how the slow requests fall in time: how many come in
clusters (another slow request due within 100 us), the gaps between clusters, and the share of
slow requests by the due time's offset within a millisecond (a tick would show as a peak).
"""
import argparse
import json
import struct
import sys
from collections import Counter, defaultdict
from pathlib import Path


def runs(path):
    for line in open(path):
        if line.strip():
            r = json.loads(line)
            if r.get("type") == "run":
                yield r


def load(raw):
    b = Path(raw).read_bytes()
    out = []
    for due, lat, seg, flags in struct.iter_unpack("<QQHH", b):
        out.append((due, lat, seg, flags))
    return out


def q(xs, p):
    if not xs:
        return 0.0
    i = min(len(xs) - 1, int(p * len(xs)))
    return xs[i]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("rawdir")
    ap.add_argument("--slow", type=float, default=42.0)
    ap.add_argument("--rate-segs", default="s70,t")
    a = ap.parse_args()
    # The raw files are named by the server's pid, which benchrun's record does not keep; the
    # session runs one server at a time, so the files in the order written are the runs in
    # order (seq). A run that failed before the load generator wrote would shift this, so the
    # counts are checked.
    raws = sorted(Path(a.rawdir).glob("*.raw"), key=lambda p: p.stat().st_mtime)
    allruns = sorted(runs(a.results), key=lambda r: r.get("seq", 0))
    if len(raws) != len(allruns):
        sys.exit(f"{len(raws)} raw files for {len(allruns)} runs: match them by hand")
    for r, match in zip(allruns, raws):
        if not r.get("served"):
            continue
        segs = r["served"]["generator"]["segments"]
        names = [s["name"] for s in segs]
        recs = load(match)
        n70 = r["served"]["groups"]["s70"]["lat_us"]["n"]
        if sum(1 for x in recs if names[x[2]] == "s70") != n70:
            sys.exit(f"{match.name}: s70 count differs from {r['allocator']}'s record")
        by = defaultdict(list)
        for due, lat, seg, flags in recs:
            by[names[seg]].append(lat / 1e3)
        line = []
        for n in names:
            if n.startswith("_") or n in ("sat", "off", "idle") or n in [x.split()[0] for x in line]:
                continue
            xs = sorted(by[n])
            if not xs:
                continue
            slow = sum(1 for x in xs if x > a.slow) / len(xs)
            line.append(f"{n} {q(xs, .99):4.1f}/{q(xs, .999):4.1f}/{100 * slow:4.2f}%")
        print(f"\n{r['allocator']} ({match.name}) p99/p99.9/share>{a.slow:.0f}us")
        for i in range(0, len(line), 7):
            print("  " + "  ".join(line[i:i + 7]))
        pre = tuple(a.rate_segs.split(","))
        hot = [(due, lat / 1e3) for due, lat, seg, flags in recs if names[seg].startswith(pre)]
        slow = [(d, l) for d, l in hot if l > a.slow]
        if not slow:
            continue
        clustered = sum(1 for i, (d, _) in enumerate(slow)
                        if (i > 0 and d - slow[i - 1][0] < 100_000) or (i + 1 < len(slow) and slow[i + 1][0] - d < 100_000))
        # Cluster: slow requests each due within 100 us of the previous.
        clusters = []
        for d, l in slow:
            if clusters and d - clusters[-1][-1][0] < 100_000:
                clusters[-1].append((d, l))
            else:
                clusters.append([(d, l)])
        sizes = Counter(min(len(c), 10) for c in clusters)
        gaps = sorted((clusters[i + 1][0][0] - clusters[i][0][0]) / 1e6 for i in range(len(clusters) - 1))
        span = (hot[-1][0] - hot[0][0]) / 1e9
        print(f"  slow over the 70% pieces: {len(slow)} of {len(hot)} ({100 * len(slow) / len(hot):.2f}%), "
              f"{len(clusters)} clusters ({len(clusters) / span:.0f}/s), {100 * clustered / len(slow):.0f}% in clusters; "
              f"cluster sizes {dict(sorted(sizes.items()))}")
        if gaps:
            print(f"  gaps between clusters ms: p10 {q(gaps, .1):.2f} p50 {q(gaps, .5):.2f} p90 {q(gaps, .9):.2f}")
        ms = Counter(int((d % 1_000_000) / 100_000) for d, _ in slow)
        print("  slow by due offset within a ms (tenths): " + " ".join(f"{ms[i]}" for i in range(10)))
        worst = sorted(slow, key=lambda x: -x[1])[:5]
        print("  slowest: " + ", ".join(f"{l:.0f}us@{d / 1e9:.4f}s" for d, l in worst))


if __name__ == "__main__":
    main()
