#!/usr/bin/env python3
"""One table a job from where.py's and misses.py's JSON outputs: cycles a request by path for
each allocator (the mean over the runs given), and the share of the server's cycles.

  summarize.py JSON...     (where-*.json and misses-*.json files, any order)

Rows group where.py's categories; a category that is the allocator's in one allocator's runs
and absent in the other's reads as zero there.
"""
import collections
import json
import sys

ROWS = [
    ("fast path: allocate", lambda c: c in ("asmalloc: fast: alloc", "jemalloc: fast: alloc")),
    ("fast path: free", lambda c: c in ("asmalloc: fast: free", "jemalloc: fast: free")),
    ("realloc (incl. its copy)", lambda c: c.endswith(": realloc") or "realloc (libc" in c),
    ("slow path: exchange, flush, other", lambda c: c.startswith(("asmalloc: slow:", "jemalloc: slow:"))
     and "fill" not in c and "launder" not in c),
    ("slow path: fill, launder", lambda c: c.startswith(("asmalloc: slow:", "jemalloc: slow:"))
     and ("fill" in c or "launder" in c)),
    ("large objects", lambda c: c.endswith(": large") or c.endswith(": large (libc copy/zero)")),
    ("maintenance and purge (request threads)", lambda c: c.startswith(("asmalloc: maint", "jemalloc: maint"))),
    ("other allocator code (copies, zeroing)", lambda c: (c.startswith(("asmalloc:", "jemalloc:"))
     and "(libc copy/zero)" in c and "realloc" not in c and "large" not in c)),
    ("kernel: faults in allocator code", lambda c: c.startswith("kernel: page fault, allocator")),
    ("kernel: madvise, mprotect, munmap, mmap", lambda c: c.startswith(("kernel: madvise", "kernel: mprotect",
                                                                         "kernel: munmap", "kernel: mmap",
                                                                         "kernel: mremap", "kernel: brk"))),
    ("kernel: membarrier", lambda c: c.startswith("kernel: membarrier")),
    ("kernel: futex from allocator", lambda c: c == "kernel: futex, allocator"),
    ("kernel: other syscalls from allocator", lambda c: c == "kernel: other syscalls, allocator"),
    ("background thread", lambda c: c.startswith("background thread")),
    ("kernel: page faults in program code", lambda c: c == "kernel: page fault, program"),
    ("program, user", lambda c: c == "program (user)"),
    ("kernel, other", lambda c: c.startswith("kernel:")),
    ("unknown", lambda c: True),
]
ALLOCATOR_ROWS = 14  # the first rows are the allocator's own


def row_of(cat):
    for i, (name, test) in enumerate(ROWS):
        if test(cat):
            return i
    return len(ROWS) - 1


def main():
    where = collections.defaultdict(list)
    misses = collections.defaultdict(list)
    for f in sys.argv[1:]:
        j = json.load(open(f))
        alloc = j["alloc"].replace("-dbg", "").replace("-prof", "")
        if "categories" in j:
            where[(j["job"], alloc)].append(j)
        elif "per_request" in j:
            misses[(j["job"], alloc)].append(j)
    jobs = sorted({job for job, _ in where} | {job for job, _ in misses})
    for job in jobs:
        allocs = sorted({a for j, a in where if j == job})
        print(f"\n### {job}\n")
        print("| | " + " | ".join(f"{a} cycles/req | {a} %" for a in allocs) + " |")
        print("|---|" + "---:|---:|" * len(allocs))
        tables = {}
        for a in allocs:
            runs = where[(job, a)]
            acc = collections.Counter()
            total = 0.0
            for r in runs:
                total += r["cycles"] / r["requests"]
                for c, w in r["categories"].items():
                    acc[row_of(c)] += w / r["requests"]
            n = len(runs)
            tables[a] = ({k: v / n for k, v in acc.items()}, total / n, n)
        printed_total = False
        for i, (name, _) in enumerate(ROWS):
            if i >= ALLOCATOR_ROWS and not printed_total:
                cells = []
                for a in allocs:
                    s = sum(tables[a][0].get(k, 0.0) for k in range(ALLOCATOR_ROWS))
                    cells += [f"**{s:,.0f}**", f"**{100 * s / tables[a][1]:.2f}**"]
                print("| **allocator, all of the above** | " + " | ".join(cells) + " |")
                printed_total = True
            vals = [tables[a][0].get(i, 0.0) for a in allocs]
            if not any(v > 0 for v in vals):
                continue
            cells = []
            for a, v in zip(allocs, vals):
                cells += [f"{v:,.0f}", f"{100 * v / tables[a][1]:.2f}"]
            print(f"| {name} | " + " | ".join(cells) + " |")
        cells = []
        for a in allocs:
            cells += [f"{tables[a][1]:,.0f}", f"{tables[a][2]} runs"]
        print("| server, all | " + " | ".join(cells) + " |")
        if any(j == job for j, _ in misses):
            print(f"\nmisses a request ({job}):\n")
            keys = ["LOAD program heap", "STORE program heap", "LOAD allocator heap", "STORE allocator heap",
                    "LOAD allocator other anon", "STORE allocator other anon"]
            cols = ["L1 miss", "from L3", "from other core", "from DRAM", "dTLB L1 miss", "page walk"]
            ma = sorted({a for j, a in misses if j == job})
            print("| access | " + " | ".join(f"{a}: {c}" for a in ma for c in cols) + " |")
            print("|---|" + "---:|" * (len(cols) * len(ma)))
            for k in keys:
                cells = []
                for a in ma:
                    runs = misses[(job, a)]
                    for c in cols:
                        cells.append(f"{sum(r['per_request'].get(k, {}).get(c, 0.0) for r in runs) / len(runs):.1f}")
                print(f"| {k} | " + " | ".join(cells) + " |")


if __name__ == "__main__":
    main()
