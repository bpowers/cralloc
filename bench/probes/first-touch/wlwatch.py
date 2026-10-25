#!/usr/bin/env python3
"""Reads the address-space trace (wl.rs) out of the servers a benchrun session starts, for
docs/research/first-touch.md, part 2. Every PERIOD seconds, for each live process whose
executable exports `ASMALLOC_WL`, it copies the records written since its last look from the
process's memory and appends them to OUT/<pid>.wl (48 bytes each, as wl.rs lays them out), and
notes the name of every thread it has not seen (OUT/<pid>.threads, `tid name` lines). Reading
another process's memory takes that process's address-space lock for reading, so the period is
long and each read is of the new records only.

  wlwatch.py OUT --exe PATH [--exe PATH...] [--period 1.0]

Runs until SIGTERM or SIGINT; start it beside the session, inside the same scripts/measure, on
the runner's CPU.
"""
import argparse
import os
import signal
import struct
import subprocess
import time
from pathlib import Path

REC = 48


def locate(pid, exe):
    nm = subprocess.run(["nm", exe], capture_output=True, text=True).stdout
    sym = [l.split() for l in nm.splitlines() if l.endswith(" ASMALLOC_WL")]
    if not sym:
        return None
    value = int(sym[0][0], 16)
    for line in Path(f"/proc/{pid}/maps").read_text().splitlines():
        f = line.split()
        if len(f) >= 6 and f[5] == exe and int(f[2], 16) == 0:
            return int(f[0].split("-")[0], 16) + value
    return None


class Proc:
    def __init__(self, pid, addr, out):
        self.pid, self.addr = pid, addr
        self.mem = open(f"/proc/{pid}/mem", "rb", buffering=0)
        self.out = open(out / f"{pid}.wl", "ab")
        self.thr = open(out / f"{pid}.threads", "a", buffering=1)
        self.seen = 0
        self.tids = set()
        self.mem.seek(addr + 8)
        self.len = struct.unpack("<Q", self.mem.read(8))[0]
        self.lost = 0

    def poll(self):
        self.mem.seek(self.addr)
        n = struct.unpack("<Q", self.mem.read(8))[0]
        if n - self.seen > self.len:
            self.lost += n - self.seen - self.len
            self.seen = n - self.len
        # Records still being written (word 0 zero) are left for the next look.
        while self.seen < n:
            i = self.seen % self.len
            k = min(n - self.seen, self.len - i)
            self.mem.seek(self.addr + 16 + i * REC)
            data = self.mem.read(k * REC)
            good = 0
            for j in range(k):
                if struct.unpack_from("<Q", data, j * REC)[0] == 0:
                    break
                good += 1
            self.out.write(data[:good * REC])
            self.seen += good
            if good < k:
                break
        self.out.flush()
        try:
            for t in os.listdir(f"/proc/{self.pid}/task"):
                if t not in self.tids:
                    self.tids.add(t)
                    name = Path(f"/proc/{self.pid}/task/{t}/comm").read_text().strip()
                    self.thr.write(f"{t} {name}\n")
        except OSError:
            pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--exe", action="append", required=True)
    ap.add_argument("--period", type=float, default=1.0)
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    exes = {os.path.realpath(e) for e in a.exe}
    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append(1))
    signal.signal(signal.SIGINT, lambda *_: stop.append(1))
    procs, skip = {}, set()
    while not stop:
        t_next = time.monotonic() + a.period
        for d in os.listdir("/proc"):
            if not d.isdigit() or d in procs or d in skip:
                continue
            try:
                exe = os.path.realpath(f"/proc/{d}/exe")
            except OSError:
                continue
            if exe not in exes:
                skip.add(d)
                continue
            addr = locate(d, exe)
            if addr is None:
                skip.add(d)
                continue
            procs[d] = Proc(d, addr, out)
            with open(out / "procs", "a") as f:
                f.write(f"{d} {exe} {time.time():.3f} {time.monotonic_ns()}\n")
        for d, p in list(procs.items()):
            try:
                p.poll()
            except (OSError, ValueError, struct.error):
                with open(out / "procs", "a") as f:
                    f.write(f"{d} gone lost={p.lost}\n")
                del procs[d]
        time.sleep(max(0.0, t_next - time.monotonic()))
    for d, p in procs.items():
        try:
            p.poll()
        except (OSError, ValueError, struct.error):
            pass


if __name__ == "__main__":
    main()
