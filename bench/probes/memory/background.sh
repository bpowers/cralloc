#!/usr/bin/env bash
# The background thread's evidence in docs/research/memory.md ("The background thread"): the
# quiet run whose light work never leaves the magazines, with the thread off and on and against
# jemalloc, natively and through the preload library, interleaved; then a summary.
# usage: background.sh <worktree> <cpu> <rounds>
# Needs: build.sh <worktree>/bench cralloc bg=cralloc-background je sys=
#        cargo build --release -p cralloc-preload, and bench/baselines/build.sh for jemalloc.
set -u
root="$1"; cpu="$2"; rounds="$3"
out="${OUT:-$root/bench/target/memory-out/background}"
mkdir -p "$out"
lib="$root/target/release/libcralloc_preload.so"
je="${JE:-$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)/../bench/baselines/out/jemalloc/libjemalloc.so}"
t="$root/bench/target"
args=(quiet live=2M sizes=small secs=16 after=pairs after_sizes=small)
run() {
  local name=$1 round=$2; shift 2
  BENCH_CPUS=$cpu MEMLIMIT_QUIET=1 "$root/scripts/memlimit" -m 4G -t 5m -- \
    "$@" "${args[@]}" >"$out/$name.$round.log" 2>&1 || echo "$name round $round failed"
}
for round in $(seq 1 "$rounds"); do
  run rust-off "$round" env -u CRALLOC_BACKGROUND "$t/cralloc/release/workload"
  run rust-on "$round" env -u CRALLOC_BACKGROUND "$t/bg/release/workload"
  run rust-je "$round" env -u CRALLOC_BACKGROUND "$t/je/release/workload"
  run pre-off "$round" env -u CRALLOC_BACKGROUND LD_PRELOAD="$lib" "$t/sys/release/workload"
  run pre-on "$round" env CRALLOC_BACKGROUND=1 LD_PRELOAD="$lib" "$t/sys/release/workload"
  run pre-je "$round" env -u CRALLOC_BACKGROUND LD_PRELOAD="$je" "$t/sys/release/workload"
done
# Median anonymous resident MiB over the rounds, after the frees and at each time shown.
python3 -I - "$out" <<'EOF'
import pathlib, re, statistics, sys
out = pathlib.Path(sys.argv[1])
at = [2, 4, 6, 8, 12, 16]
print("config    freed " + " ".join(f"{s:>5}s" for s in at))
for name in ["rust-off", "rust-on", "rust-je", "pre-off", "pre-on", "pre-je"]:
    runs = []
    for log in sorted(out.glob(f"{name}.*.log")):
        text = log.read_text(errors="replace")
        freed = re.search(r"phase=freed t_ms=\d+ rss_anon=(\d+)", text)
        after = [(int(t), int(b)) for t, b in re.findall(r"phase=after t_ms=(\d+) rss_anon=(\d+)", text)]
        if not freed or not after:
            continue
        t0 = after[0][0] - 250
        row = [int(freed.group(1))]
        for s in at:
            row.append([b for t, b in after if t - t0 <= s * 1000 + 10][-1])
        runs.append(row)
    if runs:
        med = [statistics.median(col) / 2**20 for col in zip(*runs)]
        print(f"{name:8} " + " ".join(f"{v:6.1f}" for v in med) + f"  (n={len(runs)})")
EOF
