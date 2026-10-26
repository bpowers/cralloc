#!/usr/bin/env python3
"""A burst's requests beside the allocator's address-space calls, from a first-touch session
(session.sh, `wl_edit.py` build): every request of the segments whose name starts with `b`
in its first MS milliseconds, with its latency, and every traced call from 100 ms before it.

    python3 -I bench/probes/huge-tail/burstwl.py OUTDIR TAG [--ms 600]
"""
import json
import struct
import sys
from pathlib import Path

OPS = {1: "mprotect", 2: "dontneed", 3: "hugepage", 4: "nohugepage", 5: "madv-other", 6: "munmap",
       7: "mmap", 8: "mincore"}
CTX = {0: "-", 1: "commit", 2: "to-huge", 3: "pass", 4: "hold", 5: "growth", 6: "trim", 7: "move",
       8: "resize", 9: "free", 10: "alloc"}


def main():
    d, tag = Path(sys.argv[1]), sys.argv[2]
    ms = float(sys.argv[sys.argv.index("--ms") + 1]) if "--ms" in sys.argv else 600.0
    runs = [json.loads(l) for l in open(d / f"{tag}.jsonl")]
    gen = next(r["served"]["generator"] for r in runs if (r.get("served") or {}).get("generator"))
    start = gen["start_mono_ns"]
    segs = gen["segments"]
    first = min(i for i, s in enumerate(segs) if s["name"].startswith("b"))
    t_burst = start + segs[first]["begin_ms"] * 1e6
    for wl in sorted((d / f"{tag}.wl").glob("*.wl")):
        pid = wl.stem
        names = {}
        tf = wl.with_suffix(".threads")
        if tf.exists():
            for line in tf.read_text().splitlines():
                t, _, n = line.partition(" ")
                names[int(t)] = n
        data = wl.read_bytes()
        calls = []
        for t0, w1, ln, w3, cpu, sw in struct.iter_unpack("<QQQQQQ", data[: len(data) // 48 * 48]):
            if t_burst - 100e6 <= t0 <= t_burst + ms * 1e6:
                calls.append((t0, w1 & 0xffffffff, names.get(w1 >> 32, w1 >> 32), ln, w3 & 0xff, (w3 >> 8) & 0xff, cpu))
        raw = d / f"{tag}.raw" / f"{pid}.raw"
        b = raw.read_bytes() if raw.exists() else b""
        reqs = [(start + due, lat) for due, lat, seg, fl in struct.iter_unpack("<QQHH", b[: len(b) // 20 * 20])
                if t_burst <= start + due <= t_burst + ms * 1e6]
        events = [(c[0], "call", c) for c in calls] + [(r[0], "req", r) for r in reqs]
        events.sort()
        print(f"== {tag} pid {pid}: {len(reqs)} requests, {len(calls)} calls")
        for t, kind, e in events:
            off = (t - t_burst) / 1e6
            if kind == "req":
                if e[1] >= 5e6:
                    print(f"  {off:8.2f} ms  request {e[1] / 1e6:6.1f} ms")
            else:
                print(f"  {off:8.2f} ms  {OPS.get(e[4], e[4])} {e[3] >> 10}K by {e[2]} ({CTX.get(e[5], e[5])}) "
                      f"{e[1] / 1e3:.0f} us cpu {e[6]}")


if __name__ == "__main__":
    main()
