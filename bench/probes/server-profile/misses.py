#!/usr/bin/env python3
"""Data-cache and TLB misses per request by what memory they touched and whose code touched it,
from a `record.py --mode ibs` recording (IBS op samples with data addresses, and the page
faults recorded beside them).

  misses.py DIR [--rate R] [--json OUT]

IBS tags one op in every PERIOD ops, so each sample stands for PERIOD ops of the server; per
request is count * PERIOD / requests. For each sampled load or store of the server:

- the memory: `heap` (the allocator's: for asmalloc its reservation, for any allocator the
  anonymous mappings that are no thread's stack), `stack` (a mapping that holds a thread's
  stack pointer, from stacks.json, or [stack]), `glibc heap` ([heap], C code's malloc),
  `file` (the binary, libraries, mapped files) or `kernel`;
- whose instruction: the allocator's (asmalloc's inline chain or rseq routine, jemalloc's
  functions) or the program's;
- where it was served (L1, L2, L3, another core's cache, DRAM) and whether it missed the L1
  and L2 TLBs (a page walk), with the miss latency IBS reports.

Page faults are counted by address class from faults.data.
"""
import argparse
import bisect
import collections
import json
import os
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elfinfo
from where import RATES, Symbolizer, asm_path, je_range, load_base



def mappings(maps):
    out = []
    for line in maps.splitlines():
        parts = line.split(None, 5)
        lo, hi = (int(x, 16) for x in parts[0].split("-"))
        path = parts[5].strip() if len(parts) > 5 else ""
        out.append((lo, hi, path))
    return out


class Classes:
    def __init__(self, maps, stacks, is_asm):
        self.m = sorted(mappings(maps))
        self.lo = [lo for lo, _, _ in self.m]
        sps = [sp for _, sp in stacks.values()]
        self.stack = set()
        for i, (lo, hi, path) in enumerate(self.m):
            if path == "[stack]" or any(lo <= sp < hi for sp in sps):
                self.stack.add(i)
        # asmalloc's reservation: the largest run of adjacent anonymous mappings.
        self.res = None
        if is_asm:
            best, run_lo, prev_hi = 0, None, None
            for lo, hi, path in self.m:
                if path == "" and prev_hi == lo and run_lo is not None:
                    pass
                elif path == "":
                    run_lo = lo
                else:
                    run_lo = None
                if path == "":
                    prev_hi = hi
                    if hi - run_lo > best:
                        best, self.res = hi - run_lo, (run_lo, hi)
                else:
                    prev_hi = None

    def of(self, addr):
        if addr >= 0xFFFF800000000000:
            return "kernel"
        if self.res and self.res[0] <= addr < self.res[1]:
            return "heap"
        i = bisect.bisect_right(self.lo, addr) - 1
        if i < 0 or addr >= self.m[i][1]:
            return "unmapped"
        lo, hi, path = self.m[i]
        if i in self.stack:
            return "stack"
        if path == "[heap]":
            return "glibc heap"
        if path == "":
            return "other anon" if self.res else "heap"
        if path.startswith("["):
            return path
        return "file"


def level(lvl, snp):
    if lvl.startswith("L1"):
        return "L1"
    if lvl.startswith("L2"):
        return "L2"
    if lvl.startswith("LFB"):
        return "LFB"
    if "HitM" in snp or "Remote core" in lvl:
        return "other core"
    if lvl.startswith("L3"):
        return "L3"
    if "RAM" in lvl:
        return "DRAM"
    return lvl or "?"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--rate", type=float)
    ap.add_argument("--json")
    a = ap.parse_args()
    d = Path(a.dir)
    meta = json.loads((d / "meta.json").read_text())
    binary = meta["server"]
    maps = (d / "maps.txt").read_text()
    stacks = json.loads((d / "stacks.json").read_text()) if (d / "stacks.json").exists() else {}
    base = load_base(maps, binary)
    rate = a.rate or RATES[meta["job"]]
    requests = rate * meta["dur"]
    period = meta["period"]
    is_asm = "asmalloc" in meta["alloc"]
    cls = Classes(maps, stacks, is_asm)
    secs = elfinfo.sections(open(binary, "rb").read())
    t = secs[".text"]
    text_lo, text_hi = base + t[0], base + t[0] + t[2]
    sym = Symbolizer(binary)
    je_lo, je_hi = (None, None) if is_asm else je_range(binary)

    p = subprocess.Popen(["perf", "script", "-i", str(d / "perf.data"), "-F",
                          "comm,pid,tid,addr,weight,data_src,ip,data_page_size" if meta.get("weights", True)
                          else "comm,pid,tid,addr,data_src,ip,data_page_size", "--no-inline"],
                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, errors="replace")
    rows = []
    all_ips = []
    ops = 0
    bad = 0
    for line in p.stdout:
        if not line.strip():
            continue
        parts = line.rstrip("\n").split("|")
        if len(parts) < 7:
            bad += 1
            continue
        head = parts[0].split()
        # comm may hold spaces: pid/tid is the first token with a slash and digits around it.
        j = next((i for i, x in enumerate(head) if re.fullmatch(r"\d+/\d+", x)), None)
        if j is None:
            bad += 1
            continue
        pid = int(head[j].split("/")[0])
        if pid != meta["server_pid"]:
            continue
        ops += 1
        op = parts[1].replace("OP", "", 1).strip()
        all_ips.append(int(parts[6].split()[-2], 16))
        if op not in ("LOAD", "STORE"):
            continue
        nums = head[j + 1:]
        addr = int(nums[0], 16)
        lvl = parts[2].replace("LVL", "", 1).strip()
        snp = parts[3].replace("SNP", "", 1).strip()
        tlb = parts[4].replace("TLB", "", 1).strip()
        tail = parts[6].split()
        ip = int(tail[-2], 16)
        # The miss latency IBS reports is printed before the address when the access missed.
        w = int(tail[-3]) if len(tail) >= 3 and re.fullmatch(r"\d+", tail[-3]) else 0
        rows.append((op, addr, w, lvl, snp, tlb, ip, tail[-1]))
    p.wait()
    sym.resolve([ip - base for ip in all_ips if text_lo <= ip < text_hi])

    def owner(ip):
        if ip >= 0xFFFF800000000000:
            return "kernel"
        if not text_lo <= ip < text_hi:
            return "program"
        chain = sym[ip - base]
        if is_asm:
            return "allocator" if asm_path(chain) else "program"
        return "allocator" if je_lo is not None and je_lo <= ip - base < je_hi else "program"

    k = period / requests
    by_owner = collections.Counter(owner(ip) for ip in all_ips)
    print("ops by code (IBS, precise):", ", ".join(f"{o} {100 * n / len(all_ips):.2f}%" for o, n in by_owner.most_common()))
    acc = collections.Counter()
    lat = collections.Counter()
    pages = collections.Counter()
    for op, addr, w, lvl, snp, tlb, ip, psize in rows:
        who = owner(ip)
        where = cls.of(addr)
        lv = level(lvl, snp)
        key = (op, who, where)
        acc[key + ("all",)] += 1
        if lv != "L1":
            acc[key + ("L1 miss",)] += 1
            acc[key + ("from " + lv,)] += 1
            lat[key] += w
        if tlb.startswith("L2 hit"):
            acc[key + ("dTLB L1 miss",)] += 1
        elif tlb.startswith("L2 miss") or "Walker" in tlb:
            acc[key + ("dTLB L1 miss",)] += 1
            acc[key + ("page walk",)] += 1
        if where == "heap":
            pages[(who, psize)] += 1

    print(f"{meta['job']} / {meta['alloc']}: {ops} op samples, one per {period} ops, "
          f"{ops * period / requests:,.0f} ops a request; {len(rows)} loads and stores ({bad} lines unparsed)")
    if is_asm and cls.res:
        print(f"asmalloc's reservation: {cls.res[0]:#x}-{cls.res[1]:#x}")
    kinds = ["all", "L1 miss", "from L2", "from L3", "from other core", "from DRAM", "from LFB", "dTLB L1 miss",
             "page walk"]
    print(f"\nper request (count * {period} / {requests:,.0f} requests):")
    print(f"  {'op':5} {'code':9} {'memory':11}" + "".join(f"{x:>14}" for x in kinds) + f"{'miss lat sum':>14}")
    groups = sorted({key[:3] for key in acc}, key=lambda g: -acc[g + ("L1 miss",)])
    out = {}
    for g in groups:
        vals = [acc[g + (x,)] * k for x in kinds]
        if acc[g + ("all",)] < 3:
            continue
        print(f"  {g[0]:5} {g[1]:9} {g[2]:11}" + "".join(f"{v:14.2f}" for v in vals) + f"{lat[g] * k:14.0f}")
        out[" ".join(g)] = dict(zip(kinds, vals), miss_latency=lat[g] * k)
    print("\nheap loads and stores by page size (samples):", dict(pages))

    faults = collections.Counter()
    fpath = d / "faults.data"
    if fpath.exists():
        fp = subprocess.Popen(["perf", "script", "-i", str(fpath), "-F", "pid,addr,ip", "--no-inline"],
                              stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, errors="replace")
        for line in fp.stdout:
            parts = line.split()
            if len(parts) < 3 or int(parts[0]) != meta["server_pid"]:
                continue
            addr = int(parts[1], 16)
            ip = int(parts[2], 16)
            faults[(cls.of(addr), owner(ip))] += 1
        fp.wait()
        print("page faults per request by memory and code:",
              {f"{w} {o}": round(n / requests, 5) for (w, o), n in faults.most_common()},
              f"({sum(faults.values())} in {meta['dur']} s)")
    if a.json:
        Path(a.json).write_text(json.dumps({"job": meta["job"], "alloc": meta["alloc"], "requests": requests,
                                            "per_request": out,
                                            "ops_by_code": {o: n / len(all_ips) for o, n in by_owner.items()}, "faults": {f"{w} {o}": n for (w, o), n in faults.items()},
                                            "heap_page_sizes": {f"{w} {s}": n for (w, s), n in pages.items()}},
                                           indent=1))


if __name__ == "__main__":
    main()
