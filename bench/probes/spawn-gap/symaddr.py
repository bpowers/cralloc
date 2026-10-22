#!/usr/bin/env python3
"""Address, size, offset in its 64-byte line and in its 4 KiB page of the functions whose
demangled names contain each PATTERN, in each binary.
usage: symaddr.py BIN[,BIN...] PATTERN..."""
import subprocess, sys

bins = sys.argv[1].split(",")
pats = sys.argv[2:]
for b in bins:
    out = subprocess.run(["nm", "-C", "-S", "--defined-only", b], capture_output=True, text=True).stdout
    print("==", b.split("/target/")[-1])
    for line in out.splitlines():
        f = line.split(" ", 3)
        if len(f) < 4 or f[2] not in "tT":
            continue
        name = f[3]
        if any(p in name for p in pats):
            a, s = int(f[0], 16), int(f[1], 16)
            print(f"  {a:#9x} size {s:5} line+{a % 64:2} page+{a % 4096:4} end-line+{(a + s) % 64:2}  {name[:110]}")
