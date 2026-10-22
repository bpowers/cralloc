"""dfsum.py SET: DataFusion runs of set SET (bench/target/hc-runs/df/SET-BUILD-rN.*): per build,
each run's cgroup peak, faults and cycles, and the medians; for diagnostic builds, the
CREDITDIAG totals (MiB) and, from the series, the state at the sample of most anonymous memory."""
import glob
import os
import re
import statistics
import sys
from collections import defaultdict

O = "/home/bpowers/src/asmalloc/.claude/worktrees/agent-ac0a2195f8109ecfc/bench/target/hc-runs/df"
MIB = ("carve_small", "carve_large", "refault_large", "refault_small", "owed", "paid1", "paid2",
       "dropped", "hold", "decay", "avoidable", "avoidable_whole", "cover")
COLS = ["t_ms", "anon", "debt", "ret_small", "ret_large", "carve_small", "carve_large",
        "refault_large", "refault_small", "paid1", "paid2", "hold", "decay", "dropped", "cover"]

builds = defaultdict(list)
for peak in sorted(glob.glob(f"{O}/{sys.argv[1]}-*.peak")):
    tag = os.path.basename(peak)[:-5]
    m = re.match(rf"{re.escape(sys.argv[1])}-(.+)-r(\d+)$", tag)
    if m:
        builds[m.group(1)].append((int(m.group(2)), tag))

for b, runs in sorted(builds.items()):
    peaks, faults, cycles = [], [], []
    for r, tag in sorted(runs):
        p = int(open(f"{O}/{tag}.peak").read().split()[0]) / 1048576
        out = open(f"{O}/{tag}.out").read()
        pf = re.search(r'"page_faults":(\d+)', out)
        cy = re.search(r'"cycles":(\d+)', out)
        if not (pf and cy):
            print(f"{b} r{r}: no result")
            continue
        peaks.append(p)
        faults.append(int(pf.group(1)))
        cycles.append(int(cy.group(1)))
        line = f"{b:5s} r{r} peak {p:7.1f} faults {int(pf.group(1)):8d} Gcyc {int(cy.group(1)) / 1e9:6.2f}"
        err = open(f"{O}/{tag}.err").read()
        if "WARNING" in err:
            line += " MEMLIMIT-WARNING"
        print(line)
        d = re.findall(r"CREDITDIAG[^\n]*", err)
        if d:
            kv = d[-1].split()[1:]
            d = dict(zip(kv[::2], (int(x) for x in kv[1::2])))
            print("      ", " ".join(f"{k} {d[k] / 1048576:.0f}" if k in MIB else f"{k} {d[k]}" for k in d))
        sf = f"{O}/{tag}.series"
        if os.path.exists(sf):
            rows = [list(map(float, l.split())) for l in open(sf) if l.strip()]
            if rows:
                top = max(rows, key=lambda x: x[1])
                print("       at most anon:", " ".join(f"{c} {v:.0f}" for c, v in zip(COLS, top)),
                      f"| max debt {max(x[2] for x in rows):.0f} samples {len(rows)}")
    if peaks:
        print(f"{b:5s} median peak {statistics.median(peaks):7.1f} ({min(peaks):.0f} to {max(peaks):.0f})"
              f" faults {statistics.median(faults) / 1e3:7.0f}k ({min(faults) / 1e3:.0f}k to {max(faults) / 1e3:.0f}k)"
              f" Gcyc {statistics.median(cycles) / 1e9:6.2f}")
