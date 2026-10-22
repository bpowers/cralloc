#!/usr/bin/env python3
"""Where a server's cycles went, from a `record.py --mode cycles` recording: user time in the
allocator by path, kernel time by cause, the rest of the program, per request.

  where.py DIR [--rate R] [--json OUT] [--top N]

Samples are the server's own (its pid), weighted by their period (cycles). Each is put in one
category:

- asmalloc, user: every sampled address of the server binary is named with its inline chain
  (`llvm-symbolizer --inlining` on the binary, which must have line tables: rpc-server has, the
  apps are measured as their line-table builds). A sample whose chain includes asmalloc's
  source is the allocator's, put in a path by the frames in the chain (fast path, the exchange
  of magazines, fill, launder, maintenance and purge, large objects, realloc). The inlined pop
  and push routines are also found by address from the binary's rseq descriptors
  (elfinfo.py), as a check on the line tables.
- jemalloc, user: by function name (`_rjem_*`); it is C, called out of line.
- memcpy, memmove and memset in libc are the allocator's when the call into them came from
  allocator code: the last user branch from the server binary into libc or its PLT, from the
  sample's branch record (LBR).
- kernel: by the kernel call chain: a page fault, or the system call it is in (madvise,
  mprotect, munmap, membarrier, futex, ...), or interrupts. A page fault is the allocator's
  when the faulting user instruction is the allocator's, else the program's first touch. A
  system call is the allocator's when the instruction that made it is the allocator's (asmalloc
  makes its own) or the last call from the server binary before it was (jemalloc's, through
  libc).
- The background thread (comm `asmalloc`) is reported apart.

Requests are the rate (from the job, or --rate) times the recording's length.
"""
import argparse
import collections
import json
import os
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import elfinfo

RATES = {"orders axum": 235_000, "orders axum h2": 166_000, "connect log": 85_000,
         "connect log h2": 63_000, "connect log grpc h2": 63_000, "orders axum cache 2G": 244_000,
         "connect store 1M": 36_300, "meilisearch hn": 930}

FRAME = re.compile(r"^\s+([0-9a-f]+) (.*) \((.*)\)$")
HEAD = re.compile(r"^(.+?)\s+(\d+)/(\d+)\s+\[(\d+)\]\s+([\d.]+):\s+(\d+)\s+(\S+):")
LEAF = re.compile(r"^\s*(.+?)\s+(\d+)/(\d+)\s+\[(\d+)\]\s+([\d.]+):\s+(\d+)\s+(\S+):\s+([0-9a-f]+) (.*?) \(([^()]*)\)(.*)$")
BRANCH = re.compile(r"0x([0-9a-f]+) \(([^)]*)\)/0x([0-9a-f]+) \(([^)]*)\)/")
FIELDS = "comm,pid,tid,cpu,time,period,event,ip,sym,dso"


def popen(args):
    return subprocess.Popen(["perf", "script", *args], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                            text=True, errors="replace", bufsize=1 << 20)


def chains(perf_data):
    """Each sample with its frame-pointer call chain (whole in the kernel; the user side is
    the instruction that entered the kernel and whatever frame pointers give)."""
    p = popen(["-i", str(perf_data), "-F", FIELDS, "--no-inline"])
    cur = None
    for line in p.stdout:
        if not line.strip():
            if cur:
                yield cur
            cur = None
            continue
        if line[0] not in " \t":
            m = HEAD.match(line)
            if m:
                cur = {"comm": m.group(1).strip(), "pid": int(m.group(2)), "tid": int(m.group(3)),
                       "key": (float(m.group(5)), int(m.group(4))), "period": int(m.group(6)), "frames": []}
            continue
        if cur is not None:
            m = FRAME.match(line)
            if m:
                cur["frames"].append((int(m.group(1), 16), m.group(2), m.group(3)))
    if cur:
        yield cur
    p.wait()


def leaves(perf_data):
    """Each sample's own address and its branch record, most recent branch first."""
    p = popen(["-i", str(perf_data), "-F", FIELDS + ",brstack", "--no-inline", "-G"])
    for line in p.stdout:
        m = LEAF.match(line)
        if m:
            br = [(int(a, 16), da, int(b, 16), db) for a, da, b, db in BRANCH.findall(m.group(11))]
            yield (float(m.group(5)), int(m.group(4))), int(m.group(2)), (int(m.group(8), 16), m.group(9), m.group(10)), br
    p.wait()


def samples(perf_data):
    """Both passes, merged by (time, CPU): both are in time order, and a line either pass
    cannot parse only drops that sample's extra information."""
    lv = leaves(perf_data)
    pending = next(lv, None)
    for s in chains(perf_data):
        while pending is not None and pending[0] < s["key"]:
            pending = next(lv, None)
        leaf, br = None, []
        if pending is not None and pending[0] == s["key"]:
            _, _, leaf, br = pending
            pending = next(lv, None)
        if leaf is not None and (not s["frames"] or s["frames"][0][0] != leaf[0]):
            s["frames"] = [leaf]
        s["branches"] = br
        yield s


def load_base(maps, binary):
    for line in maps.splitlines():
        parts = line.split()
        if len(parts) >= 6 and parts[5] == binary and int(parts[2], 16) == 0:
            return int(parts[0].split("-")[0], 16)
    return None


class Symbolizer:
    """Inline chains for addresses of one binary: [(function, file), ...], innermost first."""

    def __init__(self, binary):
        self.binary = binary
        self.cache = {}

    def resolve(self, addrs):
        todo = sorted(a for a in set(addrs) if a not in self.cache)
        if not todo:
            return
        inp = "".join(f"{a:#x}\n" for a in todo)
        out = subprocess.run(["llvm-symbolizer", "--inlining", "--functions=linkage", "--demangle",
                              f"--obj={self.binary}"], input=inp, capture_output=True, text=True).stdout
        for a, b in zip(todo, out.split("\n\n")):
            lines = b.strip().splitlines()
            self.cache[a] = [(lines[i], lines[i + 1].rsplit(":", 2)[0]) for i in range(0, len(lines) - 1, 2)]

    def __getitem__(self, a):
        return self.cache.get(a, [])


ASM_FILE = "/crates/asmalloc/src/"


def asm_path(chain):
    """The allocator path of an inline chain that includes asmalloc's code, else None."""
    files = [f for _, f in chain if ASM_FILE in f]
    if not files:
        return None
    names = " ".join(fn for fn, f in chain if ASM_FILE in f)
    rel = [f.split(ASM_FILE, 1)[1] for f in files]
    has = lambda *xs: any(x in names for x in xs)
    in_file = lambda *xs: any(r.startswith(xs) for r in rel)
    if has("run_thread", "::maintain"):
        return "background"
    if in_file("back/purge.rs", "back/hold.rs") or has("purge"):
        return "maint: purge"
    if has("heartbeat", "::beat", "::paced", "idle_look", "age_and_sweep", "launder_overflow", "between_beats"):
        if has("launder"):
            return "maint: launder"
        return "maint: other"
    if has("GlobalAlloc>::realloc", "::resize", "move_large"):
        return "realloc"
    if in_file("heap/large.rs") or has("alloc_large", "free_large", "dealloc_other", "alloc_other", "grant_other",
                                         "free_by_address", "alloc_staggered"):
        return "large"
    if has("launder"):
        return "slow: launder"
    if has("::fill", "Source>::fill", "fill_from", "claim"):
        return "slow: fill"
    if has("rust_alloc_slow", "rust_free_slow", "alloc_slow", "free_slow", "exchange_for", "obtain_", "retire",
           "to_depot", "pop_full", "spare_", "centre_for", "store_if_equal", "c_alloc_small", "c_free_small"):
        return "slow: exchange"
    if in_file("depot/", "back/", "os/"):
        return "slow: other"
    if has("dealloc", "free_small", "push", "free"):
        return "fast: free"
    return "fast: alloc"


def je_range(binary):
    """The address range of jemalloc's code in a binary: tikv-jemalloc-sys links it as one
    static archive, so its functions, static ones included, lie together between its first and
    last exported (`_rjem_`) symbols."""
    lo = hi = None
    out = subprocess.run(["nm", "-n", "-S", binary], capture_output=True, text=True).stdout
    for line in out.splitlines():
        parts = line.split()
        if len(parts) == 4 and parts[2] in "tT" and parts[3].startswith("_rjem_"):
            a, size = int(parts[0], 16), int(parts[1], 16)
            lo = a if lo is None else min(lo, a)
            hi = a + size if hi is None else max(hi, a + size)
    return lo, hi


def je_path(sym, inside=False):
    """jemalloc's path for one of its functions; `inside` when the address is known to lie in
    jemalloc's code (static functions carry no prefix)."""
    if not (inside or sym.startswith("_rjem_") or "jemalloc" in sym or sym.startswith("je_")):
        return None
    t = sym.replace("_rjem_je_", "").replace("_rjem_", "")
    if re.search(r"rallocx|xallocx|ralloc|realloc", t):
        return "realloc"
    if re.search(r"decay|purge|extent_dalloc_wrapper|ehooks|pages_purge|madvise", t):
        return "maint: purge"
    if re.search(r"^large_|_large|huge", t):
        return "large"
    if re.search(r"tcache_bin_flush|tcache_gc|tcache_event|tcache_flush", t):
        return "slow: flush"
    if re.search(r"tcache_alloc_small_hard|cache_bin_fill|arena_cache_bin_fill|bin_fill", t):
        return "slow: fill"
    if re.search(r"mutex", t):
        return "slow: lock"
    if re.search(r"^(sdallocx|free)$", t):
        return "fast: free"
    if re.search(r"^(malloc|mallocx|calloc|nallocx|sallocx)$", t):
        return "fast: alloc"
    if re.search(r"^(malloc|mallocx|sdallocx|free|calloc)\.cold$", t):
        return "slow: other"
    return "slow: other"


KERNEL_SYSCALL = re.compile(r"__(?:x64|ia32)_sys_(\w+)|__do_sys_(\w+)")
FAULT = ("asm_exc_page_fault", "exc_page_fault", "do_user_addr_fault", "handle_mm_fault")
COPY = ("__memmove", "__memcpy", "__memset", "memcpy", "memmove", "memset", "__GI_memcpy", "__GI_memset")
ALLOC_SYSCALLS = ("madvise", "mprotect", "munmap", "mmap", "mremap", "membarrier", "brk")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--rate", type=float)
    ap.add_argument("--json")
    ap.add_argument("--top", type=int, default=25)
    a = ap.parse_args()
    d = Path(a.dir)
    meta = json.loads((d / "meta.json").read_text())
    binary = meta["server"]
    base = load_base((d / "maps.txt").read_text(), binary)
    if base is None:
        sys.exit("no mapping of the server binary in maps.txt")
    rate = a.rate or RATES[meta["job"]]
    requests = rate * meta["dur"]
    ranges = elfinfo.Ranges(binary)
    secs = elfinfo.sections(open(binary, "rb").read())
    text = secs[".text"]
    text_lo, text_hi = base + text[0], base + text[0] + text[2]
    is_asm = "asmalloc" in meta["alloc"]
    je_lo, je_hi = (None, None) if is_asm else je_range(binary)

    raw = [s for s in samples(d / "perf.data") if s["pid"] == meta["server_pid"]]
    sym = Symbolizer(binary)
    addrs = []
    for s in raw:
        for ip, _, dso in s["frames"][:8]:
            if dso == binary:
                addrs.append(ip - base)
        for f, df, t, dt in s["branches"]:
            if text_lo <= f < text_hi:
                addrs.append(f - base)
    sym.resolve(addrs)

    def path_at(ip):
        """The allocator path of an address in the server binary's text, or None."""
        if not text_lo <= ip < text_hi:
            return None
        chain = sym[ip - base]
        if is_asm:
            return asm_path(chain)
        name = chain[-1][0] if chain else ""
        if je_lo is not None and je_lo <= ip - base < je_hi:
            return je_path(name, inside=True)
        if "__rust_" in name and "dealloc" in name:
            return "fast: free"
        if "__rust_" in name and "alloc" in name:
            return "fast: alloc"
        return None

    def caller_of_leaf(br):
        """The call site in the server binary of the libc function the sample is in: the most
        recent recorded branch from the binary's text to anywhere outside it (libc, or the
        binary's PLT)."""
        for f, df, t, dt in br:
            if text_lo <= f < text_hi and not (text_lo <= t < text_hi):
                return f
        return None

    cat = collections.Counter()
    syms = collections.defaultdict(collections.Counter)
    check = collections.Counter()
    nobr = collections.Counter()
    total = 0
    for s in raw:
        w = s["period"]
        total += w
        fr = s["frames"]
        bg = s["comm"] == "asmalloc"
        if not fr:
            cat["unknown"] += w
            continue
        if fr[0][2].startswith("["):
            names = [n for _, n, dso in fr if dso.startswith("[")]
            user = [f for f in fr if not f[2].startswith("[")]
            sc = None
            for n in names:
                m = KERNEL_SYSCALL.match(n)
                if m:
                    sc = m.group(1) or m.group(2)
                    break
            fault = any(n in FAULT for n in names)
            entry = user[0][0] if user else None
            if fault:
                owner = path_at(entry) if entry else None
                c = "kernel: page fault, " + (f"allocator ({owner})" if owner else "program")
            elif sc:
                owner = path_at(entry) if entry else None
                if not owner:
                    site = caller_of_leaf(s["branches"])
                    owner = path_at(site) if site else None
                if sc in ALLOC_SYSCALLS:
                    c = f"kernel: {sc}, " + ("allocator" if owner else "program")
                elif sc == "futex":
                    c = "kernel: futex, " + ("allocator" if owner else "program")
                else:
                    c = "kernel: other syscalls" + (", allocator" if owner else "")
            elif any("irq" in n or "softirq" in n or "sysvec" in n for n in names):
                c = "kernel: interrupts"
            else:
                c = "kernel: other"
            if bg:
                c = "background thread: " + c
            cat[c] += w
            syms[c][names[0] if names else "?"] += w
            continue
        ip, name, dso = fr[0]
        p = None
        if dso == binary:
            k = ranges.kind(ip - base)
            p = path_at(ip)
            if k in ("pop", "push"):
                check[(k, p)] += w
            chain = sym[ip - base]
            label = " < ".join(fn.split("(")[0][-60:] for fn, f in chain[:3]) if chain else name
        elif "libc" in dso and name.startswith(COPY):
            site = caller_of_leaf(s["branches"])
            if site is None:
                nobr[name] += w
                label = f"{name} < (no call in the branch record)"
            else:
                p = path_at(site)
                chain = sym[site - base]
                label = f"{name} < " + (" < ".join(fn.split("(")[0][-50:] for fn, f in chain[:2]) if chain else "?")
                if p:
                    p = p + " (libc copy/zero)"
        else:
            label = f"{name} ({os.path.basename(dso)})"
        if bg:
            c = "background thread: " + (p or "other")
        elif p:
            c = ("asmalloc: " if is_asm else "jemalloc: ") + p
        else:
            c = "program (user)"
        cat[c] += w
        syms[c][label] += w

    per = lambda x: x / requests
    print(f"{meta['job']} / {meta['alloc']}: {len(raw)} samples, {total / 1e9:.2f} G cycles over "
          f"{meta['dur']} s at {rate:.0f}/s: {per(total):,.0f} cycles a request")
    rows = sorted(cat.items(), key=lambda kv: -kv[1])
    for c, w in rows:
        print(f"  {c:58} {per(w):10,.0f}  {100 * w / total:6.2f}%")
    groups = collections.Counter()
    for c, w in cat.items():
        groups[c.split(":")[0]] += w
    print("groups:", ", ".join(f"{g} {100 * w / total:.2f}%" for g, w in groups.most_common()))
    if check:
        print("rseq ranges against line tables:", {k: f"{100 * v / total:.2f}%" for k, v in check.items()})
    if nobr:
        print("libc copies with no call in the branch record:", {k: f"{100 * v / total:.2f}%" for k, v in nobr.items()})
    for c, _ in rows:
        print(f"\n{c}:")
        for l, w in syms[c].most_common(a.top if c != "program (user)" else max(a.top, 30)):
            print(f"  {100 * w / total:6.2f}%  {l[:160]}")
    if a.json:
        Path(a.json).write_text(json.dumps({"job": meta["job"], "alloc": meta["alloc"], "requests": requests,
                                            "rate": rate, "dur": meta["dur"], "cycles": total,
                                            "categories": dict(cat),
                                            "top": {c: syms[c].most_common(40) for c in cat}}, indent=1))


if __name__ == "__main__":
    main()
