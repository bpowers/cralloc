#!/usr/bin/env python3
"""Whether two binaries have the same code at the same addresses: every allocated section
(code, read-only data, the rseq descriptors) byte for byte, and its address. A line-table twin
(build-twin.sh) may stand in for the measured binary when naming samples only if this passes.

  textcmp.py MEASURED TWIN
"""
import struct
import sys

sys.path.insert(0, __import__("os").path.dirname(__file__))
from elfinfo import sections

SHF_ALLOC = 2


def alloc_sections(path):
    data = open(path, "rb").read()
    shoff, = struct.unpack_from("<Q", data, 0x28)
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", data, 0x3A)
    names = sections(data)
    out = {}
    for name, (addr, off, size, typ) in names.items():
        # Allocated sections have an address; debug sections do not.
        if addr and typ != 8:
            out[name] = (addr, data[off:off + size])
        elif addr:
            out[name] = (addr, b"\0" * 0)
    return out


a, b = alloc_sections(sys.argv[1]), alloc_sections(sys.argv[2])
bad = 0
for name in sorted(set(a) | set(b)):
    if name not in a or name not in b:
        print(f"{name}: only in {'first' if name in a else 'second'}")
        bad += 1
    elif a[name] != b[name]:
        same_addr = a[name][0] == b[name][0]
        print(f"{name}: differs (address {'same' if same_addr else 'moved'}, sizes {len(a[name][1])} / {len(b[name][1])})")
        bad += 1
print("identical" if not bad else f"{bad} sections differ")
sys.exit(1 if bad else 0)
