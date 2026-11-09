#!/usr/bin/env python3
"""What ran beside a server job, by process, and which of its runs were quiet: whoran.py DIR

Reads the pieces of a `quiet-session.sh` session, each a benchrun results file NAME.jsonl beside
NAME.perf, a machine-wide log of context switches (`perf record -a --switch-events -e dummy`).
Prints, for the server's CPUs and the generator's while a generator ran:

- each group's CPU time, runs over 0.5 and 1 ms and longest run: kernel threads, Chromium,
  `claude` and its tools, other desktop, and the runner. The job itself is left out.
- the processes behind them and the longest runs.
- each run's tail at the ranked rate beside the longest other run in that segment. A run with
  no other run over 0.5 ms there is quiet, and only such a run's tail can be read as the allocator's
  (docs/research/measuring.md, "On the M1 Max").
- the median p99.9 over the quiet runs, by job and allocator.

Run it under scripts/heavy: a piece takes about two minutes, four at a time.
"""
import argparse
import collections
import json
import multiprocessing
import re
import statistics
import subprocess
from pathlib import Path

LINE = re.compile(r"^\s*(.*?)\s+(-?\d+)/(-?\d+)\s+\[(\d+)\]\s+([0-9.]+):\s+PERF_RECORD_(\S+)(.*)$")
FORK = re.compile(r"\((\d+):\d+\):\((\d+):\d+\)")
COMM = re.compile(r"\s*(?:exec:\s*)?(.*?):(\d+)/(\d+)")
LONG = 500e-6


def cpu_sets():
    out = subprocess.run([str(Path(__file__).resolve().parents[2] / "scripts/cpusets")], capture_output=True, text=True)
    sets = {}
    for line in out.stdout.splitlines():
        name, _, value = line.partition(" ")
        cpus = set()
        for part in value.strip().split(","):
            if "-" in part:
                a, b = part.split("-")
                cpus.update(range(int(a), int(b) + 1))
            elif part:
                cpus.add(int(part))
        sets[name.strip()] = cpus
    return sets


class Tree:
    """The process tree from the log's own fork and comm records, which perf also writes for every
    task alive when it starts, so that even a process that lived between two glances is named."""

    def __init__(self, session_pid):
        self.parent = {}
        self.name = {}
        self.session_pid = session_pid
        self.memo = {}

    def chain(self, pid):
        out = []
        seen = set()
        while pid and pid not in seen:
            seen.add(pid)
            out.append(pid)
            pid = self.parent.get(pid, 0)
        return out

    def is_job(self, pid):
        """A descendant of the runner: the server, its upstream or the generator."""
        names = [self.name.get(p, "") for p in self.chain(pid)]
        return "benchrun" in names[1:]

    def group(self, pid):
        if pid in self.memo:
            return self.memo[pid]
        chain = self.chain(pid)
        names = [self.name.get(p, "") for p in chain]
        if pid == 2 or 2 in chain:
            g = "kernel threads"
        elif "benchrun" in names or (names and names[0] == "perf") or self.session_pid in chain:
            g = "the runner"
        elif names and names[0] == "claude":
            g = "claude"
        elif "claude" in names:
            g = "claude's tools"
        elif any(n.startswith("chromium") for n in names):
            g = "Chromium"
        else:
            g = "other desktop"
        self.memo[pid] = g
        return g

    def process(self, pid, comm):
        if self.group(pid) == "kernel threads":
            return re.split(r"[/-]", comm)[0]
        return self.name.get(pid, comm)


def read_piece(perf, session_pid):
    """Every run on a CPU by anything but idle and the job, the generators' start times and the
    process tree."""
    tree = Tree(session_pid)
    cmd = ["perf", "script", "-i", str(perf), "--show-switch-events", "--show-task-events",
           "-F", "comm,pid,tid,cpu,time,event"]
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, errors="replace")
    current = {}
    runs = []
    generators = {}
    job = {}
    for line in p.stdout:
        m = LINE.match(line)
        if not m:
            continue
        comm, pid, tid, cpu, t, kind, rest = m.groups()
        if kind == "SWITCH_CPU_WIDE":
            pid, tid, cpu, t = int(pid), int(tid), int(cpu), float(t)
            if rest.startswith(" IN"):
                current[cpu] = (t, comm, pid, tid)
            elif cpu in current and current[cpu][3] == tid:
                start, c, _, _ = current.pop(cpu)
                if pid <= 0:
                    continue
                if c == "loadgen":
                    generators.setdefault(pid, start)
                if pid not in job:
                    job[pid] = tree.is_job(pid)
                if not job[pid]:
                    runs.append((start, t, c, pid, cpu))
        elif kind.startswith("FORK"):
            f = FORK.search(kind)
            if f and f.group(1) != f.group(2):
                tree.parent[int(f.group(1))] = int(f.group(2))
        elif kind.startswith("COMM"):
            c = COMM.match(rest)
            if c and c.group(2) == c.group(3):
                tree.name[int(c.group(2))] = c.group(1)
    p.wait()
    return runs, sorted(generators.values()), tree


def summarize(args):
    perf, session_pid, sets = args
    runs, starts, tree = read_piece(perf, session_pid)
    a, b = sets["a"], sets["b"]
    results = perf.with_suffix(".jsonl")
    records = []
    if results.exists():
        records = [r for r in map(json.loads, results.open()) if r.get("type") == "run"]
    if len(records) != len(starts):
        print(f"{perf.name}: {len(records)} runs in the results but {len(starts)} generators in the log")
    windows = []
    for r, t0 in zip(records, starts):
        served = r.get("served") or {}
        marks = served.get("marks_ms") or []
        if marks:
            windows.append((t0 + marks[0][0] / 1e3, t0 + marks[-1][0] / 1e3))
    loaded = sum(y - x for x, y in windows)

    def in_load(t):
        return any(x <= t <= y for x, y in windows)

    # One row a group and cluster, and a process.
    groups = collections.defaultdict(lambda: [0.0, 0, 0, 0.0])
    procs = collections.defaultdict(lambda: [0.0, 0, 0.0, collections.Counter()])
    longest = []
    for start, end, comm, pid, cpu in runs:
        side = "server" if cpu in a else "generator" if cpu in b else None
        if side is None or not in_load(start):
            continue
        g = tree.group(pid)
        d = end - start
        w = groups[(side, g)]
        w[0] += d
        w[1] += d > LONG
        w[2] += d > 1e-3
        w[3] = max(w[3], d)
        p = procs[(g, tree.process(pid, comm))]
        p[0] += d
        p[1] += d > LONG
        p[2] = max(p[2], d)
        p[3][comm] += d
        if d > LONG:
            longest.append((d, comm, cpu, g))
    longest = sorted(longest, reverse=True)[:30]

    # Each run's tail beside what ran in its ranked segment.
    rows = []
    for r, t0 in zip(records, starts):
        served = r.get("served") or {}
        marks = served.get("marks_ms") or []
        rank = (r.get("primary") or {}).get("segment") or "s70"
        spans = [(x, y) for (x, n), (y, _) in zip(marks, marks[1:]) if n == rank]
        groups_at = (served.get("groups") or {}).get(rank)
        if not spans or not groups_at:
            continue
        w0, w1 = t0 + spans[0][0] / 1e3, t0 + spans[-1][1] / 1e3
        beside = [x for x in runs if w0 <= x[0] < w1 and (x[4] in a or x[4] in b)]
        over = [x for x in beside if x[1] - x[0] > LONG]
        top = max(beside, key=lambda x: x[1] - x[0], default=None)
        lat = groups_at["lat_us"]
        rows.append({
            "job": r["job"], "allocator": r["allocator"], "valid": r["valid"],
            "p999": lat["p999"], "max": lat["max"], "missed": groups_at["missed"],
            "over": len(over), "cpus": sum(x[1] - x[0] for x in beside) / (w1 - w0),
            "longest_ms": (top[1] - top[0]) * 1e3 if top else 0.0,
            "who": tree.group(top[3]) if top else "",
        })
    return loaded, dict(groups), {k: (v[0], v[1], v[2], v[3].most_common(4)) for k, v in procs.items()}, longest, rows


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dir", type=Path)
    a = ap.parse_args()
    pid_file = a.dir / "session.pid"
    session_pid = int(pid_file.read_text()) if pid_file.exists() else -1
    sets = cpu_sets()
    pieces = sorted(a.dir.glob("*.perf"))
    with multiprocessing.Pool(4) as pool:
        done = pool.map(summarize, [(p, session_pid, sets) for p in pieces])

    seconds = sum(d[0] for d in done)
    groups = collections.defaultdict(lambda: [0.0, 0, 0, 0.0])
    procs = collections.defaultdict(lambda: [0.0, 0, 0.0, collections.Counter()])
    longest = []
    rows = []
    for _, g, p, l, r in done:
        for k, v in g.items():
            w = groups[k]
            w[0] += v[0]
            w[1] += v[1]
            w[2] += v[2]
            w[3] = max(w[3], v[3])
        for k, v in p.items():
            w = procs[k]
            w[0] += v[0]
            w[1] += v[1]
            w[2] = max(w[2], v[2])
            w[3].update(dict(v[3]))
        longest.extend(l)
        rows.extend(r)

    print(f"{len(pieces)} pieces, {len(rows)} runs, {seconds:.0f} s under load")
    for side in ("server", "generator"):
        print(f"\nOn the {side}'s CPUs: group, CPUs, runs over 0.5 ms, over 1 ms, longest ms")
        for (s, g), w in sorted(groups.items(), key=lambda x: -x[1][0]):
            if s == side:
                print(f"  {g:16} {w[0] / max(seconds, 1):8.4f} {w[1]:7} {w[2]:7} {w[3] * 1e3:8.2f}")

    print("\nBy process: group, process, CPU s, runs over 0.5 ms, longest ms, main threads")
    for (g, name), w in sorted(procs.items(), key=lambda x: -x[1][0])[:30]:
        threads = ", ".join(f"{c} {d:.2f}" for c, d in w[3].most_common(4))
        print(f"  {g:16} {name[:20]:20} {w[0]:8.2f} {w[1]:6} {w[2] * 1e3:8.2f}  {threads}")

    print("\nLongest runs: ms, thread, CPU, group")
    for d, comm, cpu, g in sorted(longest, reverse=True)[:15]:
        print(f"  {d * 1e3:8.2f}  {comm:20} {cpu:3}  {g}")

    print("\nEach run at the ranked rate: job, allocator, p99.9 us, slowest us, other runs over 0.5 ms, "
          "other CPUs, longest other run ms, whose")
    for r in rows:
        flag = "" if r["valid"] else "  invalid"
        print(f"  {r['job']:22} {r['allocator']:16} {r['p999']:8.0f} {r['max']:8.0f} {r['over']:5} "
              f"{r['cpus']:7.4f} {r['longest_ms']:7.2f}  {r['who']}{flag}")

    quiet = [r for r in rows if r["over"] == 0 and r["valid"]]
    print(f"\nQuiet runs: {len(quiet)} of {len(rows)}. Median p99.9 over them, us (runs)")
    allocators = sorted({r["allocator"] for r in rows})
    print(f"  {'':22} " + " ".join(f"{x[:12]:>13}" for x in allocators))
    for job in sorted({r["job"] for r in rows}):
        cells = []
        for x in allocators:
            v = [r["p999"] for r in quiet if r["job"] == job and r["allocator"] == x]
            cells.append(f"{statistics.median(v):9.0f}({len(v)})" if v else f"{'':>13}")
        print(f"  {job:22} " + " ".join(f"{c:>13}" for c in cells))


if __name__ == "__main__":
    main()
