"""Helpers for the whole-block-huge drivers: the machine's huge-page state, and a sampler of
one process's huge-page-backed memory (docs/research/whole-block-huge.md)."""
import threading
import time
from pathlib import Path

# Zen 5: L1 dTLB misses of every kind, and L2 dTLB misses (page walks) for 4 KiB and 2 MiB
# pages (ls_l1_d_tlb_miss, event 0x45). User mode only.
EVENTS = "dtlb_miss=0xff45,walk_4k=0x1045,walk_2m=0x4045"


def vmstat_thp():
    out = {}
    for line in Path("/proc/vmstat").read_text().splitlines():
        k, v = line.split()
        if k.startswith("thp_") or k.startswith("compact_"):
            out[k] = int(v)
    return out


def delta(a, b):
    return {k: b[k] - a.get(k, 0) for k in b if b[k] != a.get(k, 0)}


def free_huge():
    """Free memory in blocks of 2 MiB or more (buddy orders 9 and 10), in 2 MiB pages."""
    n = 0
    for line in Path("/proc/buddyinfo").read_text().splitlines():
        f = line.split()
        counts = [int(x) for x in f[4:]]
        if len(counts) >= 11:
            n += counts[9] + 2 * counts[10]
    return n


def rollup(pid):
    out = {}
    try:
        for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
            f = line.split()
            if len(f) >= 2 and f[0].endswith(":") and f[1].isdigit():
                out[f[0][:-1]] = int(f[1])
    except OSError:
        return None
    return out


class Sampler:
    """Reads the process named by `pidfile`'s smaps_rollup every `ms` until stopped: the peak
    and mean of its anonymous and AnonHugePages memory, KiB."""

    def __init__(self, pidfile, ms=200):
        self.pidfile = Path(pidfile)
        self.ms = ms
        self.samples = []
        self.stop = threading.Event()
        self.t = threading.Thread(target=self.run, daemon=True)
        self.t.start()

    def run(self):
        pid = None
        while not self.stop.is_set():
            if pid is None:
                try:
                    pid = int(self.pidfile.read_text().strip())
                except (OSError, ValueError):
                    pid = None
            if pid is not None:
                r = rollup(pid)
                if r:
                    self.samples.append((r.get("Anonymous", 0), r.get("AnonHugePages", 0)))
            self.stop.wait(self.ms / 1000)

    def finish(self):
        self.stop.set()
        self.t.join()
        s = self.samples
        if not s:
            return {}
        return {"n": len(s), "anon_peak": max(a for a, _ in s), "anon_mean": sum(a for a, _ in s) / len(s),
                "huge_peak": max(h for _, h in s), "huge_mean": sum(h for _, h in s) / len(s)}


def pid_wrapper(pidfile):
    """A prefix that writes the PID of the command it then execs. Written for a command line
    that scripts/memlimit hands to systemd-run, which turns `$$` into `$`."""
    return ["sh", "-c", f'echo $$$$ > {pidfile}; exec "$@"', "sh"]
