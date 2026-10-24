#!/usr/bin/env python3
"""A sampler file's series, for docs/research/meili-peak.md: one line a sample, MiB.

  series.py FILE [FROM_MS TO_MS]

asmalloc (memstat_peak.rs): anonymous memory, asmalloc's resident reservation, outside it,
glibc's arenas in use, live large (asked), live large resident, free large within the hold,
free small, and the 56 MiB class's live and free. jemalloc (je_peak.rs): anonymous memory,
asked, allocated, active, dirty, glibc in use, and anonymous memory less glibc's arenas.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import account as A  # noqa: E402

MiB = 2 ** 20


def main():
    path = sys.argv[1]
    lo, hi = (float(sys.argv[2]), float(sys.argv[3])) if len(sys.argv) > 3 else (0, 1e12)
    head = open(path).readline()
    if head.startswith("jeopt") or head.startswith("je "):
        print(f"{'t_s':>7s} {'anon':>6s} {'asked':>6s} {'alloc':>6s} {'active':>6s} {'dirty':>6s} {'g_used':>6s} {'a-glibc':>7s}")
        for s in A.jestat(path):
            if lo <= s["t_ms"] <= hi:
                g = s["g_used"] + s["g_free"]
                print(f"{s['t_ms'] / 1e3:7.2f} " + " ".join(f"{v / MiB:6.0f}" for v in
                      (s["anon"], s["req"], s["allocated"], s["active"], s["pdirty"], s["g_used"]))
                      + f" {(s['anon'] - g) / MiB:7.0f}")
        return
    print(f"{'t_s':>7s} {'anon':>6s} {'resv':>6s} {'outsd':>6s} {'g_used':>6s} {'Lask':>6s} {'Lres':>6s}"
          f" {'Fhold':>6s} {'Fsmall':>6s} {'56M_l':>6s} {'56M_f':>6s}")
    for s in A.memstat(path):
        if not lo <= s["t_ms"] <= hi:
            continue
        large = [c for z, c in s["classes"].items() if z > A.SMALL_MAX]
        small = [c for z, c in s["classes"].items() if z <= A.SMALL_MAX]
        c56 = s["classes"].get(58720256, {})
        vals = (s["anon"], s["heap"], s["anon"] - s["heap"] - s["self"], s["g_used"],
                sum(c["req_slot"] for c in large), sum(c["live"] for c in large), s["flyoung"] + s["flold"],
                sum(c["free_res"] for c in small), c56.get("live", 0), c56.get("free_res", 0))
        print(f"{s['t_ms'] / 1e3:7.2f} " + " ".join(f"{v / MiB:6.0f}" for v in vals))


if __name__ == "__main__":
    main()
