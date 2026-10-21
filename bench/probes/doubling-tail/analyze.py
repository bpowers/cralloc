#!/usr/bin/env python3
"""Per-doubling breakdown of probe.py runs:

  analyze.py DIR [--each [--slow MS]] [--perf]

For every run in DIR: the doublings to the largest capacity, each with its wall and CPU time,
time off CPU, faults and context switches, and the machine's direct compactions and remote TLB
flushes around it (from the vmstat sampler). For a probe build, the allocator's timed events
on the doubling's CPU inside it: its table's allocation and free, the heartbeats they ran,
and the system calls and purge steps inside those. With --perf, where its on-CPU cycles went,
by the sampled call chains of the doubling's thread inside its window. A table per build of
medians and extremes ends the output; --each lists every doubling (over --slow ms).
"""
import argparse
import re
import statistics
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path


def rehashes(err):
    out = []
    for line in open(err):
        if line.startswith("REHASH"):
            out.append({k: int(v) for k, v in (kv.split("=") for kv in line.split()[1:])})
    top = max((r["capacity"] for r in out if r["grew"]), default=0)
    return sorted((r for r in out if r["grew"] and r["capacity"] == top), key=lambda r: r["start_ns"])


def vmstat(path):
    rows = []
    if not path.exists():
        return rows
    for line in open(path):
        f = line.split()
        rows.append((int(f[0]), {k: int(v) for k, v in (kv.split("=") for kv in f[1:]) if v}))
    return rows


def delta(rows, a, b, key):
    before = [r for t, r in rows if t <= a]
    after = [r for t, r in rows if t >= b]
    if not before or not after:
        return None
    return after[0].get(key, 0) - before[-1].get(key, 0)


def classify(chain):
    """A label for a sample from its call chain, leaf first."""
    syms = [s for _, s in chain]
    text = " ".join(syms)
    kernel = [s for s in syms if not s.startswith("[unknown]")]
    if "asm_exc_page_fault" in text or "exc_page_fault" in text:
        for key, label in (("compact_zone", "fault:compaction"), ("try_to_free", "fault:reclaim"),
                           ("shrink_", "fault:reclaim"), ("clear_page", "fault:zeroing"),
                           ("rwsem_down", "fault:mmap_lock wait"), ("lock_mm_and_find_vma", "fault:mmap_lock"),
                           ("__pte_alloc", "fault:page table"), ("charge", "fault:memcg"),
                           ("get_page_from_freelist", "fault:page alloc"), ("rmqueue", "fault:page alloc"),
                           ("lru", "fault:lru"), ("flush_tlb", "fault:tlb")):
            if any(key in s for s in syms[:12]):
                return label
        return "fault:other"
    for key, label in (("__x64_sys_mprotect", "sys:mprotect"), ("__x64_sys_madvise", "sys:madvise"),
                       ("__x64_sys_mmap", "sys:mmap"), ("__x64_sys_munmap", "sys:munmap"),
                       ("__x64_sys_futex", "sys:futex"), ("flush_tlb_func", "irq:tlb flush"),
                       ("sysvec_call_function", "irq:call function"), ("sysvec_apic_timer", "irq:timer"),
                       ("entry_SYSCALL", "sys:other")):
        if key in text:
            return label
    leaf = syms[0] if syms else "?"
    if "asmalloc" in leaf or "back::" in leaf or "heap::" in leaf or "depot::" in leaf:
        return "user:asmalloc " + short(leaf)
    if "je_" in leaf or "jemalloc" in leaf or "_rjem" in leaf:
        return "user:jemalloc"
    if "memcpy" in leaf or "memmove" in leaf or "memset" in leaf:
        return "user:" + short(leaf)
    if "hashbrown" in leaf or "reserve_rehash" in leaf or "HashMap" in leaf or "insert" in leaf:
        return "user:rehash"
    return "user:" + short(leaf)


def short(sym):
    sym = re.sub(r"<[^<>]*>", "", sym)
    sym = re.sub(r"<[^<>]*>", "", sym)
    return sym.split("::h")[0][-60:]


def perf_samples(data):
    """(time ns, tid, event, chain [(addr, sym)]) from perf script."""
    if not data.exists():
        return []
    txt = subprocess.run(["perf", "script", "-i", str(data), "-F", "tid,time,event,ip,sym", "--ns"],
                         capture_output=True, text=True).stdout
    out = []
    cur = None
    for line in txt.splitlines():
        if not line.strip():
            if cur:
                out.append(cur)
            cur = None
            continue
        if not line.startswith("\t") and not line.startswith(" " * 8):
            m = re.match(r"\s*(\d+)\s+(\d+)\.(\d+):\s+(?:\d+\s+)?(\S+):", line)
            if m:
                tid, s, ns, ev = m.groups()
                cur = (int(s) * 10**9 + int(ns), int(tid), ev, [])
            continue
        if cur is not None:
            parts = line.strip().split(None, 1)
            if len(parts) == 2:
                cur[3].append((parts[0], parts[1]))
    if cur:
        out.append(cur)
    return out


KINDS = {1: "alloc", 2: "alloc-beat", 3: "free", 4: "free-beat", 5: "mprotect", 6: "madvise",
         7: "pay_growth", 8: "hold_beat"}


def events(path):
    """(tsc start, ticks, kind, cpu, arg) of a probe build's ring, oldest first."""
    import struct
    if not path.exists():
        return []
    raw = path.read_bytes()
    ev = []
    for i in range(0, len(raw), 32):
        t, dur, kc, arg = struct.unpack_from("<4Q", raw, i)
        if t:
            ev.append((t, dur, kc & 0xff, kc >> 8, arg))
    return sorted(ev)


def tsc_rate(rs):
    """Ticks per ns, from the doublings' (tsc, monotonic) pairs."""
    if len(rs) < 2:
        return None
    a, b = rs[0], rs[-1]
    return (b["tsc"] - a["tsc"]) / (b["start_ns"] - a["start_ns"])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--each", action="store_true")
    ap.add_argument("--perf", action="store_true")
    ap.add_argument("--slow", type=float, default=0.0, help="detail only doublings over this many ms")
    args = ap.parse_args()
    d = Path(args.dir)
    by_build = defaultdict(list)
    for err in sorted(d.glob("*.server.err")):
        tag = err.name[: -len(".server.err")]
        build = tag.rsplit("-", 1)[0]
        rs = rehashes(err)
        vm = vmstat(d / f"{tag}.vmstat")
        samples = perf_samples(d / f"{tag}.perf.data") if args.perf else []
        ev = events(d / f"{tag}.events")
        rate = tsc_rate(rs) if ev else None
        print(f"== {tag}: {len(rs)} doublings" + (f", {len(ev)} probe events, {rate:.4f} ticks/ns" if ev else ""))
        own = defaultdict(list)
        perf_total = Counter()
        perf_n = 0
        for r in rs:
            a, b = r["start_ns"], r["start_ns"] + r["ns"]
            r["compact"] = delta(vm, a - 5_000_000, b + 5_000_000, "compact_stall")
            r["tlbflush"] = delta(vm, a - 5_000_000, b + 5_000_000, "tlb_remote_flush")
            by_build[build].append(r)
            cs = [s for s in samples if s[1] == r["tid"] and a <= s[0] <= b]
            if samples:
                perf_n += 1
                for s in cs:
                    if s[2].startswith("cycles"):
                        perf_total[classify(s[3])] += 1
            mine_ev = []
            if ev:
                t0 = r["tsc"]
                t1 = t0 + r["ns"] * rate
                inside = [e for e in ev if t0 <= e[0] <= t1]
                # The doubling's own calls: its table's allocation and free, and what ran on
                # the same CPU inside them.
                spans = [e for e in inside if e[2] in (1, 3) and e[4] in (0,) + tuple(range(1 << 20, 1 << 23))]
                cpus = {e[3] for e in spans}
                mine_ev = [e for e in inside if e[3] in cpus]
                for e in mine_ev:
                    own[KINDS[e[2]]].append(e[1] / rate / 1e3)
                r["alloc_us"] = sum(e[1] for e in mine_ev if e[2] == 1) / rate / 1e3
                r["mine_ev"] = mine_ev
            if args.each and r["ns"] / 1e6 >= args.slow:
                print(f"  t={a / 1e9:.3f} tid={r['tid']} ms={r['ns'] / 1e6:.3f} cpu={r['cpu_ns'] / 1e6:.3f}"
                      f" off={(r['ns'] - r['cpu_ns']) / 1e6:.3f} faults={r['faults']} vcsw={r['vcsw']}"
                      f" ivcsw={r['ivcsw']} compact={r['compact']} tlbflush={r['tlbflush']}")
                for e in r.get("mine_ev", []):
                    print(f"     ev +{(e[0] - r['tsc']) / rate / 1e3:7.1f}us cpu{e[3]} {KINDS[e[2]]} {e[1] / rate / 1e3:.1f}us arg={e[4]}")
                if cs:
                    cnt = Counter(classify(s[3]) for s in cs if s[2].startswith("cycles"))
                    sw = [s for s in cs if s[2].startswith("context")]
                    print("     perf: " + ", ".join(f"{k} {v}" for k, v in cnt.most_common(8)))
                    for s in sw:
                        print("     switch: " + " <- ".join(short(x[1]) for x in s[3][:8]))
        if own:
            print("  allocator events on the doublings' CPUs inside them (us): kind n median max total/doubling")
            for k, v in sorted(own.items()):
                v.sort()
                print(f"    {k:11s} {len(v):4d} {statistics.median(v):8.1f} {v[-1]:8.1f} {sum(v) / max(len(rs), 1):8.1f}")
        if perf_total:
            tot = sum(perf_total.values())
            print(f"  perf over {perf_n} doublings, {tot} cycle samples:")
            for k, v in perf_total.most_common(25):
                print(f"    {v / tot * 100:5.1f}%  {v / perf_n:6.1f}/doubling  {k}")
    print()
    print("build        n   ms med  ms p90  ms max  cpu med  off med  off max  faults  ivcsw  vcsw  compact")
    for build, rs in sorted(by_build.items()):
        ms = sorted(r["ns"] / 1e6 for r in rs)
        cpu = [r["cpu_ns"] / 1e6 for r in rs]
        off = [(r["ns"] - r["cpu_ns"]) / 1e6 for r in rs]
        print(f"{build:10s} {len(rs):3d}  {statistics.median(ms):6.3f}  {ms[int(len(ms) * 0.9)]:6.3f}  {ms[-1]:6.3f}"
              f"  {statistics.median(cpu):7.3f}  {statistics.median(off):7.3f}  {max(off):7.3f}"
              f"  {statistics.median(r['faults'] for r in rs):6.0f}  {sum(r['ivcsw'] for r in rs):5d}"
              f"  {sum(r['vcsw'] for r in rs):4d}  {sum(r['compact'] or 0 for r in rs):7d}")


if __name__ == "__main__":
    main()
