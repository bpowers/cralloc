#!/usr/bin/env python3
"""Instructions by function, or by inline chain, from a histogram of stepcount.c.

usage: stepsum.py OUT PROGRAM [--per N | --per-most NAME] [--depth D] [--min X]

OUT is what bench/tools/stepcount.c wrote for PROGRAM. Prints each function's instructions, most
first. --per divides them by N, to read a count an operation. --per-most divides by how often the
most-run instruction of the functions whose names contain NAME ran, which is one a call of a function
with no loop, and one an object of a job's loop that does not spin. --depth prints the chain of inlined
functions down to D levels instead, which needs line tables and llvm-symbolizer. --min leaves out rows
under X.

Rows that a clock drives are not to be read directly: docs/research/measuring.md, "Exact instruction
counts on aarch64".
"""
import argparse
import bisect
import collections
import re
import subprocess
import sys


def clean(name):
    """A function's name with its crate and generic arguments off, to fit a line."""
    name = re.sub(r"\bcralloc::", "", name)
    before = None
    while name != before:
        before = name
        name = re.sub(r"(?:(?<=\w)|::)<[^<>]*>", "", name)
    return name


def chains(addresses, program):
    """Each address's inline frames, outermost first."""
    text = "\n".join(hex(a) for a in addresses) + "\n"
    out = subprocess.run(
        ["llvm-symbolizer", "--inlining", "--demangle", f"--obj={program}"],
        input=text,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    found = {}
    for address, block in zip(addresses, out.split("\n\n")):
        lines = block.strip().split("\n")
        found[address] = [clean(lines[i]) for i in range(len(lines) - 2, -1, -2)]
    return found


def main():
    ap = argparse.ArgumentParser(usage=__doc__.split("\n\n")[1].removeprefix("usage: "))
    ap.add_argument("out")
    ap.add_argument("program")
    ap.add_argument("--per", type=float)
    ap.add_argument("--per-most")
    ap.add_argument("--depth", type=int)
    ap.add_argument("--min", type=float, default=0)
    args = ap.parse_args()

    counts = {}
    with open(args.out) as f:
        for line in f:
            address, count = line.split()
            counts[int(address, 16)] = int(count)
    symbols = []
    nm = subprocess.run(["nm", "-C", args.program], capture_output=True, text=True, check=True).stdout
    for line in nm.splitlines():
        fields = line.split(None, 2)
        if len(fields) == 3 and fields[1] in "tTwW":
            symbols.append((int(fields[0], 16), fields[2]))
    symbols.sort()
    starts = [s[0] for s in symbols]

    def function(address):
        at = bisect.bisect_right(starts, address) - 1
        return symbols[at][1] if address and at >= 0 else "(outside the program)"

    per = args.per or 1
    if args.per_most:
        hits = [c for a, c in counts.items() if args.per_most in function(a)]
        if not hits:
            sys.exit(f"stepsum: no function that ran has {args.per_most!r} in its name")
        per = max(hits)
    total = sum(counts.values())
    print(f"{total} instructions, {total / per:.1f} a unit, the unit counted {per:g} times")

    if args.depth:
        tree = {}
        frames = chains(sorted(a for a in counts if a), args.program)
        for address, count in counts.items():
            node = tree
            for frame in frames.get(address, ["(outside the program)"])[: args.depth]:
                entry = node.setdefault(frame, [0, {}])
                entry[0] += count
                node = entry[1]

        def show(node, indent):
            for frame, (count, below) in sorted(node.items(), key=lambda kv: -kv[1][0]):
                if count / per >= args.min:
                    print(f"{count / per:10.1f} {'  ' * indent}{frame[:90]}")
                    show(below, indent + 1)

        show(tree, 0)
        return
    by = collections.Counter()
    for address, count in counts.items():
        by[clean(function(address))] += count
    for name, count in by.most_common():
        if count / per >= args.min:
            print(f"{count / per:10.1f} {name[:100]}")


main()
