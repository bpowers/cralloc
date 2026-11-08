#!/usr/bin/env python3
"""Checks that a linked binary's restartable sequences stayed as they were written: bodies.py FILE...

A linker working around Cortex-A53 erratum 843419 can replace a load after an `adrp` near a page's
end with a branch to a stub, and a preemption in a stub outside the sequence's range does not abort
it. This reads each descriptor in `__rseq_cs`, disassembles its range, and fails on an `adrp` in a
body or a branch that leaves the range other than to its end. Exits 1 on any, and prints them.
"""
import re
import struct
import subprocess
import sys


def sections(path):
    out = subprocess.run(["readelf", "-SW", path], capture_output=True, text=True, check=True).stdout
    found = {}
    for m in re.finditer(r"\]\s+(\S+)\s+\S+\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)", out):
        found[m.group(1)] = (int(m.group(2), 16), int(m.group(3), 16), int(m.group(4), 16))
    return found


def relative_relocations(path):
    """Addends of the relocations that fill data at load time, by the address they fill."""
    out = subprocess.run(["readelf", "-rW", path], capture_output=True, text=True, check=True).stdout
    addends = {}
    for m in re.finditer(r"^([0-9a-f]+)\s+[0-9a-f]+\s+R_AARCH64_RELATIV\w*\s+([0-9a-f]+)", out, re.M):
        addends[int(m.group(1), 16)] = int(m.group(2), 16)
    return addends


def descriptors(path):
    secs = sections(path)
    if "__rseq_cs" not in secs:
        return
    addr, off, size = secs["__rseq_cs"]
    addends = relative_relocations(path)
    data = open(path, "rb").read()[off:off + size]
    for at in range(0, size - 31, 32):
        version, flags, start, length, abort = struct.unpack_from("<IIQQQ", data, at)
        start = addends.get(addr + at + 8, start)
        abort = addends.get(addr + at + 24, abort)
        # A routine's word after its descriptor reads as an entry with no length.
        if version == 0 and flags == 0 and start and 0 < length < 4096 and abort:
            yield start, length


def check(path):
    bad = 0
    count = 0
    for start, length in descriptors(path):
        count += 1
        end = start + length
        out = subprocess.run(["objdump", "-d", "--no-show-raw-insn", f"--start-address={start}",
                              f"--stop-address={end}", path], capture_output=True, text=True, check=True).stdout
        for m in re.finditer(r"^\s*([0-9a-f]+):\s+(\S+)\s*(.*)$", out, re.M):
            at, op, args = int(m.group(1), 16), m.group(2), m.group(3)
            if op == "adrp":
                print(f"{path}: {at:#x}: an adrp in the sequence at {start:#x}")
                bad += 1
            if op == "b" or op == "bl" or op.startswith("b.") or op in ("cbz", "cbnz", "tbz", "tbnz"):
                target = re.search(r"\b([0-9a-f]+)\b(?: <|$)", args)
                if target:
                    t = int(target.group(1), 16)
                    if not start <= t <= end and op in ("b", "bl"):
                        print(f"{path}: {at:#x}: {op} out of the sequence at {start:#x}, to {t:#x}")
                        bad += 1
    print(f"{path}: {count} sequences, {bad} problems")
    return bad


sys.exit(1 if sum(check(p) for p in sys.argv[1:]) else 0)
