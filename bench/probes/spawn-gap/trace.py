#!/usr/bin/env python3
"""Load an allocation trace from bench-alloc's `trace` feature.

Each record: tsc, what (op<<56 | cpu<<40 | alignlog2<<32 | size), ptr.
"""
import sys, collections, array


def load(path):
    a = array.array("Q")
    with open(path, "rb") as f:
        a.frombytes(f.read())
    recs = []
    for i in range(0, len(a), 3):
        w = a[i + 1]
        recs.append((a[i], w >> 56, (w >> 40) & 0xFFFF, (w >> 32) & 0xFF, w & 0xFFFFFFFF, a[i + 2]))
    recs.sort(key=lambda r: r[0])
    return recs  # (tsc, op, cpu, alog, size, ptr)


if __name__ == "__main__":
    recs = load(sys.argv[1])
    allocs = [r for r in recs if r[1] in (0, 2)]
    print("events", len(recs), "allocs", len(allocs))
    c = collections.Counter((r[4], r[3]) for r in allocs)
    print("top layouts (size, align):")
    for (s, a), n in c.most_common(25):
        print(f"  size {s:6} align {1<<a:4}: {n}")
    print("first allocations:")
    for k, r in enumerate(allocs[: int(sys.argv[2]) if len(sys.argv) > 2 else 120]):
        print(f"  #{k} size {r[4]} align {1<<r[3]} ptr {r[5]:#x} cpu {r[2]} op {r[1]}")
