"""Shared by the drivers of docs/research/large-heap-constants.md: one sample of a process's
memory and of the allocator's own upkeep, read from /proc and, for a diagnostic build
(diag_edit.py), from the counters it exports.

A sample is a dict:
  t         seconds since the watch began (CLOCK_MONOTONIC)
  rss_anon  RssAnon, bytes
  pte       VmPTE, bytes
  maps      lines of /proc/PID/maps (mappings)
  cgroup    memory.current of the process's cgroup, bytes
  minflt    minor faults of the whole process
  cpu_ns    CPU time of the whole process (utime + stime), ns
  bg_ns     CPU time of the thread named `asmalloc` (the background thread), ns, from schedstat
  bg_vol    its voluntary context switches (wake-ups)
  diag      the nonzero entries of ASMALLOC_DIAG, {index: value}, if the executable has it
"""
import os
import struct
import subprocess
from pathlib import Path

TICK = os.sysconf("SC_CLK_TCK")
DIAG_N = 1024


def descendant(root, exe):
    """The process under `root` whose executable is `exe`."""
    children = {}
    for d in Path("/proc").iterdir():
        if not d.name.isdigit():
            continue
        try:
            ppid = int((d / "stat").read_text().rsplit(")", 1)[1].split()[1])
        except (OSError, IndexError, ValueError):
            continue
        children.setdefault(ppid, []).append(int(d.name))
    todo = [root]
    while todo:
        p = todo.pop()
        try:
            if os.path.realpath(f"/proc/{p}/exe") == str(exe):
                return p
        except OSError:
            pass
        todo.extend(children.get(p, []))
    return None


def symbol_address(pid, exe, name):
    """The run-time address of the executable's symbol `name` in process `pid`, or None."""
    nm = subprocess.run(["nm", str(exe)], capture_output=True, text=True).stdout
    sym = next((l.split()[0] for l in nm.splitlines() if l.endswith(" " + name)), None)
    if sym is None:
        return None
    for line in Path(f"/proc/{pid}/maps").read_text().splitlines():
        f = line.split()
        if len(f) >= 6 and f[5] == str(exe) and f[2] == "00000000":
            return int(f[0].split("-")[0], 16) + int(sym, 16)
    return None


class Watch:
    def __init__(self, pid, exe, t0):
        self.pid = pid
        self.t0 = t0
        self.diag_at = symbol_address(pid, exe, "ASMALLOC_DIAG")
        self.live_at = symbol_address(pid, exe, "BENCH_LIVE_BYTES")
        self.cg = None
        try:
            line = next(l for l in Path(f"/proc/{pid}/cgroup").read_text().splitlines() if l.startswith("0::"))
            self.cg = Path(f"/sys/fs/cgroup{line[3:].strip()}/memory.current")
        except (OSError, StopIteration):
            pass

    def _status(self):
        out = {}
        for line in Path(f"/proc/{self.pid}/status").read_text().splitlines():
            k, _, v = line.partition(":")
            if k in ("RssAnon", "VmPTE", "VmHWM"):
                out[k] = int(v.split()[0]) * 1024
        return out

    def _stat(self):
        s = Path(f"/proc/{self.pid}/stat").read_text()
        f = s[s.rindex(")") + 2:].split()
        return int(f[7]), (int(f[11]) + int(f[12])) * 1_000_000_000 // TICK

    def _background(self):
        for task in Path(f"/proc/{self.pid}/task").iterdir():
            try:
                if (task / "comm").read_text().strip() != "asmalloc":
                    continue
                ns = int((task / "schedstat").read_text().split()[0])
                vol = None
                for line in (task / "status").read_text().splitlines():
                    if line.startswith("voluntary_ctxt_switches:"):
                        vol = int(line.split()[1])
                return ns, vol
            except OSError:
                continue
        return None, None

    def _mem(self, at, n):
        with open(f"/proc/{self.pid}/mem", "rb") as f:
            f.seek(at)
            return f.read(n)

    def sample(self, now):
        try:
            st = self._status()
            minflt, cpu = self._stat()
            maps = sum(1 for _ in open(f"/proc/{self.pid}/maps"))
            bg_ns, bg_vol = self._background()
        except OSError:
            return None
        rec = {
            "t": round(now - self.t0, 3),
            "rss_anon": st.get("RssAnon"),
            "hwm": st.get("VmHWM"),
            "pte": st.get("VmPTE"),
            "maps": maps,
            "minflt": minflt,
            "cpu_ns": cpu,
            "bg_ns": bg_ns,
            "bg_vol": bg_vol,
        }
        if self.cg is not None:
            try:
                rec["cgroup"] = int(self.cg.read_text())
            except (OSError, ValueError):
                pass
        if self.diag_at is not None:
            try:
                raw = self._mem(self.diag_at, 8 * DIAG_N)
                vals = struct.unpack(f"<{DIAG_N}Q", raw)
                rec["diag"] = {i: v for i, v in enumerate(vals) if v}
            except OSError:
                pass
        if self.live_at is not None:
            try:
                b = self._mem(self.live_at, 64 * 64)
                rec["live"] = sum(int.from_bytes(b[i:i + 8], "little", signed=True) for i in range(0, len(b), 64))
            except OSError:
                pass
        return rec
