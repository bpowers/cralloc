#!/usr/bin/env python3
"""Summarizes the drift runs of docs/research/memory.md, "A steady heap does not drift" and
"When the size mix wanders, huge pages ratcheted".

    python3 -I bench/standings/drift/summary.py OUTDIR [--phases] [--series JOB]

For every OUTDIR/JOB.BUILD.txt (run.sh's output) prints, per job and build: the operations
done, the wall time of the load, cycles per operation (all threads), the ratio of resident
heap to live bytes at the end of the load, its largest value, the mean of the last quarter of
the samples against the quarter before (the trend at the end), the ratio at the end of the
quiet minute, and the peak resident heap.

With --phases, also the ratio at the end of every occurrence of each phase, in order, which
says whether a phase that comes back finds the heap where it left it. With --series JOB, every
sample of that job, one column per build.
"""
import json
import sys
from pathlib import Path

GIB = 1 << 30


def load(path):
    samples, result, exit_ok = [], None, False
    for line in path.read_text().splitlines():
        if line.startswith("DRIFT "):
            samples.append(json.loads(line[6:]))
        elif line.startswith("BENCH_RESULT "):
            result = json.loads(line[13:])
        elif line.startswith("# exit 0"):
            exit_ok = True
    return samples, result, exit_ok


def runs(outdir):
    out = {}
    for f in sorted(Path(outdir).glob("*.txt")):
        parts = f.name[: -len(".txt")].split(".")
        if len(parts) != 2:
            continue
        job, build = parts
        out.setdefault(job, {})[build] = load(f)
    return out


def mean(xs):
    return sum(xs) / len(xs) if xs else float("nan")


def summary(job, builds):
    print(f"\n## {job}\n")
    print("| build | ops | load s | cycles/op | end | max | last quarter | quarter before | quiet | peak heap GiB | huge pages at end GiB |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for build, (samples, result, ok) in sorted(builds.items()):
        load_s = [s for s in samples if s["kind"] == "load"]
        quiet = [s for s in samples if s["kind"] == "quiet"]
        if not load_s:
            print(f"| {build} | no samples | | | | | | | | |")
            continue
        r = [s["ratio"] for s in load_s]
        q = max(1, len(r) // 4)
        cyc = result["primary"]["value"] if result else float("nan")
        wall = result["phases"]["load"]["wall_ns"] / 1e9 if result else load_s[-1]["t_ms"] / 1e3
        peak = max(s["heap"] for s in samples) / GIB
        flag = "" if ok else " (unfinished)"
        print(
            f"| {build}{flag} | {load_s[-1]['ops'] / 1e9:.2f}G | {wall:.0f} | {cyc:.0f} | {r[-1]:.3f} | {max(r):.3f} | "
            f"{mean(r[-q:]):.3f} | {mean(r[-2 * q:-q]):.3f} | {quiet[-1]['ratio'] if quiet else float('nan'):.3f} | {peak:.2f} | {load_s[-1]['thp'] / GIB:.2f} |"
        )


def phase_ends(job, builds):
    print(f"\n### {job}: ratio at the end of each phase, by phase, in order\n")
    for build, (samples, _, _) in sorted(builds.items()):
        load_s = [s for s in samples if s["kind"] == "load"]
        ends = []
        for i, s in enumerate(load_s):
            if i + 1 == len(load_s) or load_s[i + 1]["phase"] != s["phase"]:
                ends.append(s)
        by = {}
        for s in ends:
            by.setdefault(s["phase"], []).append(s["ratio"])
        cells = "; ".join(f"{p}: " + " ".join(f"{x:.3f}" for x in xs) for p, xs in by.items())
        print(f"- {build}: {cells}")


def series(job, builds):
    names = sorted(builds)
    print("epoch phase " + " ".join(names))
    rows = {}
    for b in names:
        for s in builds[b][0]:
            key = (s["kind"], s["epoch"], s["t_ms"] if s["kind"] == "quiet" else 0)
            rows.setdefault((s["kind"] != "load", s["epoch"]), {}).setdefault(b, []).append(s)
    for (quiet, epoch), per in sorted(rows.items()):
        n = max(len(v) for v in per.values())
        for k in range(n):
            phase = next(iter(per.values()))[min(k, len(next(iter(per.values()))) - 1)]["phase"]
            cells = [f"{per[b][k]['ratio']:.4f}" if b in per and k < len(per[b]) else "-" for b in names]
            print(f"{epoch} {phase} " + " ".join(cells))


def main():
    args = sys.argv[1:]
    if not args:
        sys.exit(__doc__)
    data = runs(args[0])
    if "--series" in args:
        job = args[args.index("--series") + 1]
        series(job, data[job])
        return
    for job in sorted(data):
        summary(job, data[job])
        if "--phases" in args:
            phase_ends(job, data[job])


if __name__ == "__main__":
    main()
