#!/usr/bin/env python3
"""Summarize `roofline-kernels routines sweep` output against each routine's instruction layout.

    analyze_sweep.py SWEEP_OUTPUT [BINARY]

For every variant and routine, prints the offsets whose call pair or batch is slower than the
variant's fastest cell by more than half a cycle, and for each bad offset how many 64-byte lines
the hot path touches and which instructions sit in its last line. BINARY (default
target/release/roofline-kernels) supplies the instruction boundaries, from the offset-0 copy.
"""
import re
import subprocess
import sys

out = open(sys.argv[1]).read()
binary = sys.argv[2] if len(sys.argv) > 2 else "target/release/roofline-kernels"


def layout(sym):
    """(offset, length, text) of each instruction of `sym`, through its ret."""
    dis = subprocess.run(["objdump", "-d", "-M", "intel", "--no-show-raw-insn", f"--disassemble={sym}", binary],
                         capture_output=True, text=True, check=True).stdout
    rows = []
    for line in dis.splitlines():
        m = re.match(r"\s*([0-9a-f]+):\s+(\S.*)$", line)
        if m:
            rows.append((int(m.group(1), 16), m.group(2).split("#")[0].strip()))
    base = rows[0][0]
    insns = []
    for (a, t), (b, _) in zip(rows, rows[1:] + [(None, None)]):
        insns.append((a - base, (b - a) if b else 1, t))
        if t.startswith("ret"):
            break
    return insns


def describe(insns, k):
    end = insns[-1][0] + insns[-1][1]
    lines = (k + end - 1) // 64 - k // 64 + 1
    last = ((k + end - 1) // 64) * 64 - k
    tail = [t.split()[0] for o, n, t in insns if o + n > last]
    return lines, end - last, tail


for block in re.split(r"(?m)^sweep ", out)[1:]:
    head, *rows = block.strip().splitlines()
    v = head.split(":")[0]
    cells = {}
    for r in rows[1:]:
        f = r.split()
        if len(f) < 9 or not f[0].isdigit():
            continue
        k = int(f[0])
        cells[k] = [float(f[1]), float(f[3]), float(f[5]), float(f[7])]
    if not cells:
        continue
    print(f"== {v}: {head.split(':', 1)[1].split(';')[0].strip()}")
    for i, name in ((0, "pop"), (2, "push")):
        insns = layout(f"rk_{v}_{name}_00")
        for j, form in ((0, "pair"), (1, "batch")):
            col = {k: c[i + j] for k, c in cells.items()}
            best = min(col.values())
            bad = [k for k in sorted(col) if col[k] > best + 0.5]
            print(f"  {name:4} {form:5} best {best:.2f}; slower at {len(bad)} offsets")
            for k in bad:
                lines, spill, tail = describe(insns, k)
                print(f"     k={k:2} {col[k]:.2f}  lines {lines}, {spill:2} B in last line: {' '.join(tail)}")
