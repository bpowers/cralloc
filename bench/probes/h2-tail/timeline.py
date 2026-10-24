#!/usr/bin/env python3
"""timeline.py RUN [--slow US]: requests set against what ran on the CPUs, from trace.sh's
switch-event recording (RUN.srv.data, RUN.jsonl, RUN.raw/).

Builds, for every CPU of 0-3 and 8-11, the intervals in which each task ran, then for each
request recorded inside the trace (its due time and end on CLOCK_MONOTONIC) finds: how many
server CPUs were idle when it came due; whether a task other than the server's threads, the
generator's threads or idle ran on a server or generator CPU while it was in flight, and
which; and how many times a server CPU came out of idle while it was in flight. Prints these
for the slow requests and for all, and the share of slow requests among requests with and
without each condition.
"""
import argparse
import bisect
import json
import struct
import subprocess
from collections import Counter, defaultdict
from pathlib import Path

SERVER_CPUS = (0, 1, 2, 3)
CLIENT_CPUS = (8, 9, 10, 11)


def switches(path):
    """Per CPU, a sorted list of (start ns, end ns, pid, tid, comm) runs."""
    p = subprocess.Popen(["perf", "script", "-i", path, "--show-switch-events", "-F", "comm,pid,tid,cpu,time"],
                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    cur = {}
    runs = defaultdict(list)
    for line in p.stdout:
        if "PERF_RECORD_SWITCH" not in line:
            continue
        head, _, rest = line.partition("PERF_RECORD_SWITCH_CPU_WIDE")
        parts = head.split()
        i = next(k for k, x in enumerate(parts) if x.startswith("[") and x.endswith("]"))
        cpu = int(parts[i][1:-1])
        t = int(round(float(parts[i + 1].rstrip(":")) * 1e9))
        pid, _, tid = parts[i - 1].partition("/")
        comm = " ".join(parts[:i - 1])
        if rest.split()[0] == "IN":
            cur[cpu] = (t, int(pid), int(tid or pid), comm)
        else:
            if cpu in cur:
                s, cp, ct, cc = cur.pop(cpu)
                runs[cpu].append((s, t, cp, ct, cc))
    for cpu, rs in runs.items():
        rs.sort()
    return runs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("run")
    ap.add_argument("--slow", type=float, default=42.0)
    a = ap.parse_args()
    run = Path(a.run)
    rec = next(json.loads(l) for l in open(str(run) + ".jsonl") if '"type":"run"' in l.replace(" ", ""))
    base = rec["served"]["generator"]["start_mono_ns"]
    names = [s["name"] for s in rec["served"]["generator"]["segments"]]
    raw = next(Path(str(run) + ".raw").glob("*.raw"))
    spid = int(raw.stem)
    runs = switches(str(run) + ".srv.data")
    t_lo = max(rs[0][0] for c, rs in runs.items() if rs)
    t_hi = min(rs[-1][1] for c, rs in runs.items() if rs)
    starts = {c: [r[0] for r in rs] for c, rs in runs.items()}
    gen_pid = Counter(r[2] for c in CLIENT_CPUS for r in runs.get(c, [])).most_common(1)[0][0]
    others = Counter()
    for c, rs in runs.items():
        for s, e, pid, tid, comm in rs:
            if pid not in (0, spid, gen_pid):
                others[(c in SERVER_CPUS and "server cpu" or "client cpu", comm.split("/")[0])] += e - s
    print(f"trace {(t_hi - t_lo) / 1e9:.2f}s; server pid {spid}, generator pid {gen_pid}")
    print("other tasks' time on the CPUs, ms: " + ", ".join(f"{k[0]} {k[1]} {v / 1e6:.2f}" for k, v in others.most_common(12)))

    def at(c, t):
        rs = runs.get(c, [])
        i = bisect.bisect_right(starts[c], t) - 1
        return rs[i] if 0 <= i < len(rs) and rs[i][1] > t else None

    def during(c, t0, t1):
        rs = runs.get(c, [])
        i = max(0, bisect.bisect_right(starts[c], t0) - 1)
        while i < len(rs) and rs[i][0] < t1:
            if rs[i][1] > t0:
                yield rs[i]
            i += 1

    stats = defaultdict(Counter)
    foreign = Counter()
    n = 0
    for d, l, seg, fl in struct.iter_unpack("<QQHH", raw.read_bytes()):
        if fl & 4 or not names[seg].startswith(("s70", "t")):
            continue
        t0, t1 = base + d, base + d + l
        if t0 < t_lo or t1 > t_hi:
            continue
        n += 1
        slow = l > a.slow * 1e3
        idle = sum(1 for c in SERVER_CPUS if (x := at(c, t0)) is None or x[2] == 0)
        wakes = 0
        other = None
        for c in SERVER_CPUS + CLIENT_CPUS:
            for s, e, pid, tid, comm in during(c, t0, t1):
                if pid not in (0, spid, gen_pid):
                    other = f"{'srv' if c in SERVER_CPUS else 'cli'}:{comm.split('/')[0]}"
                if c in SERVER_CPUS and pid == spid and s > t0:
                    # A server thread came on this CPU while the request was in flight: from
                    # idle if the run before it was idle.
                    wakes += 1
        key = "slow" if slow else "fast"
        stats[key][f"idle={idle}"] += 1
        stats[key][f"wakes={min(wakes, 4)}"] += 1
        stats[key]["other" if other else "no other"] += 1
        if slow and other:
            foreign[other] += 1
    print(f"{n} requests in the trace, {sum(stats['slow'].values()) and stats['slow']['other'] + stats['slow']['no other']} slow")
    keys = sorted(set(stats["slow"]) | set(stats["fast"]))
    print(f"{'condition':12} {'slow':>7} {'fast':>9} {'slow share %':>13}")
    for k in keys:
        s, f = stats["slow"][k], stats["fast"][k]
        print(f"{k:12} {s:7d} {f:9d} {100 * s / max(1, s + f):13.3f}")
    print("other tasks during slow requests: " + ", ".join(f"{k} {v}" for k, v in foreign.most_common(10)))


if __name__ == "__main__":
    main()
