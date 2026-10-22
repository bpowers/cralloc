#!/usr/bin/env python3
"""What a server binary says about where asmalloc's inlined fast paths are.

  elfinfo.py BINARY            prints a summary of the binary's rseq descriptors

Every inlined copy of asmalloc's pop and push routines emits a descriptor into the binary's
`__rseq_cs` section (start, length of the critical section, abort handler), so the section
lists the instruction range of every copy, wherever LTO inlined it. In a PIE the descriptors'
addresses are filled in by R_X86_64_RELATIVE relocations, which this reads too. A routine's
three arming instructions come just before its start: in the `executable` build a 7-byte
load of the rseq offset, a 7-byte `lea` of the descriptor and a 5-byte store, 19 bytes in
all; the shared-library form loads the offset through a word of its own first (26 bytes).

Used as a module by attrib.py: `routines(path)` returns a sorted list of
(start of arming, end of commit, kind, arming bytes), kind "pop", "push" or "other" (the slow paths'
store-if-equal and spare routines, the C entry routines).
"""
import bisect
import struct
import sys

R_X86_64_RELATIVE = 8


def sections(data):
    """{name: (addr, offset, size)} from an ELF64 little-endian file's bytes."""
    shoff, = struct.unpack_from("<Q", data, 0x28)
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", data, 0x3A)
    hdrs = []
    for i in range(shnum):
        name, typ, flags, addr, off, size, link, info, align, entsize = struct.unpack_from(
            "<IIQQQQIIQQ", data, shoff + i * shentsize)
        hdrs.append((name, typ, addr, off, size))
    stroff = hdrs[shstrndx][3]
    out = {}
    for name, typ, addr, off, size in hdrs:
        end = data.index(b"\0", stroff + name)
        out[data[stroff + name:end].decode()] = (addr, off, size, typ)
    return out


def relative_relocs(data, secs):
    """{address: addend} of every R_X86_64_RELATIVE relocation."""
    out = {}
    for name, (addr, off, size, typ) in secs.items():
        if typ != 4:  # SHT_RELA
            continue
        for i in range(size // 24):
            r_off, r_info, r_add = struct.unpack_from("<QQq", data, off + 24 * i)
            if r_info & 0xFFFFFFFF == R_X86_64_RELATIVE:
                out[r_off] = r_add
    return out


def file_offset(secs, vaddr):
    for addr, off, size, typ in secs.values():
        if addr and addr <= vaddr < addr + size and typ != 8:  # not NOBITS
            return off + (vaddr - addr)
    return None


POP_MARK = bytes.fromhex("488d49f8")    # lea rcx, [rcx-8]
PUSH_MARK = bytes.fromhex("488d4108")   # lea rax, [rcx+8]
ARM_EXEC = 19


def routines(path):
    data = open(path, "rb").read()
    secs = sections(data)
    if "__rseq_cs" not in secs:
        return []
    addr, off, size, _ = secs["__rseq_cs"]
    rel = relative_relocs(data, secs)
    out = []
    for i in range(size // 32):
        a = addr + 32 * i
        version, flags, start, length, abort = struct.unpack_from("<IIQQQ", data, off + 32 * i)
        start = rel.get(a + 8, start)
        abort = rel.get(a + 24, abort)
        if not start or not length:
            continue
        fo = file_offset(secs, start)
        body = data[fo:fo + length] if fo is not None else b""
        kind = "other"
        if POP_MARK in body:
            kind = "pop"
        elif PUSH_MARK in body:
            kind = "push"
        # The executable form's arming: mov r10, [rip+x] (4c 8b 15), lea r11, [rip+y]
        # (4c 8d 1d), mov fs:[r10+8], r11 (64 4d 89 5a 08). Anything else is counted from its
        # start only.
        arm = 0
        if fo is not None and (data[fo - 19:fo - 16] == bytes.fromhex("4c8b15")
                               and data[fo - 12:fo - 9] == bytes.fromhex("4c8d1d")
                               and data[fo - 5:fo] == bytes.fromhex("644d895a08")):
            arm = ARM_EXEC
        out.append((start - arm, start + length, kind, arm))
    out.sort()
    return out


class Ranges:
    """Looks up which routine copy, if any, an address falls in."""

    def __init__(self, path):
        self.r = routines(path)
        self.starts = [s for s, _, _, _ in self.r]

    def kind(self, ip):
        i = bisect.bisect_right(self.starts, ip) - 1
        if i >= 0:
            s, e, k, _ = self.r[i]
            if ip < e:
                return k
        return None


if __name__ == "__main__":
    r = routines(sys.argv[1])
    from collections import Counter
    c = Counter((k, a) for _, _, k, a in r)
    print(f"{len(r)} descriptors, by (kind, arming bytes): {dict(c)}")
    sizes = Counter((k, e - s) for s, e, k, _ in r)
    print("lengths with arming:", dict(sorted(sizes.items())))
