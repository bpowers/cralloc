#!/usr/bin/env python3
"""Summarize `roofline-kernels routines sweep` output against each routine's instruction layout.

    analyze_sweep.py SWEEP_OUTPUT [BINARY] [--summary]

For every variant and routine, prints the offsets whose call pair or batch is slower than the
variant's fastest cell by more than half a cycle, and for each bad offset how many 64-byte lines
the hot path touches and which instructions sit in its last line. BINARY (default
target/release/roofline-kernels) supplies the instruction boundaries, from the offset-0 copy.
"""
import re
import subprocess
import sys

args = [a for a in sys.argv[1:] if not a.startswith("--")]
out = open(args[0]).read()
binary = args[1] if len(args) > 1 else "target/release/roofline-kernels"


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


def summary():
    """One row per variant: sizes, cycles with each routine at offset 0 (the other split), the
    fastest cell, and how many of the 64 offsets are within half a cycle of it."""
    print(f"{'variant':8} {'pop B/u':>8} {'push B/u':>8}  {'@0 pair':>9} {'@0 batch':>9}  "
          f"{'best':>11}  {'fast pop':>8} {'fast push':>9}  (pop/push; fast = pair and batch)")
    for block in re.split(r"(?m)^sweep ", out)[1:]:
        head, *rows = block.strip().splitlines()
        v = head.split(":")[0]
        m = re.search(r"pop (\d+) B (\d+) uops, push (\d+) B (\d+) uops", head)
        cells = {}
        for r in rows[1:]:
            f = r.split()
            if len(f) >= 9 and f[0].isdigit():
                cells[int(f[0])] = [float(f[1]), float(f[3]), float(f[5]), float(f[7])]
        if len(cells) < 64:
            continue
        best = [min(c[i] for c in cells.values()) for i in range(4)]
        fast = [sum(1 for c in cells.values() if c[i] <= best[i] + 0.5 and c[i + 1] <= best[i + 1] + 0.5)
                for i in (0, 2)]
        c0 = cells[0]
        print(f"{v:8} {m[1] + '/' + m[2]:>8} {m[3] + '/' + m[4]:>8}  {c0[0]:.2f}/{c0[2]:.2f} "
              f"{c0[1]:.2f}/{c0[3]:.2f}  {best[0]:.2f}/{best[1]:.2f}  {fast[0]:>8} {fast[1]:>9}")


if "--summary" in sys.argv:
    summary()
    sys.exit(0)

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
