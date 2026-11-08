#!/usr/bin/env bash
# The single-thread workloads under fat LTO on aarch64, one binary per allocator, each the build the
# benchmark suite runs: bench/alloc's incumbents, cralloc and the floors.
#
#   ./run_aarch64.sh build [ALLOCATOR...]
#   ./run_aarch64.sh run [ALLOCATOR...]         rounds interleaved, each under scripts/measure
#
# RF_ROUNDS (3) rounds, RA_PAD (a multiple of 16) moves each workload's loop, BUILD_TAG separates
# the build directories of different pads, RF_FILTER a workload prefix. Prints cycles a pair per
# workload and allocator, the fastest round, and +x how far the slowest round's sits above it.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
scripts=$here/../../../scripts
cd "$here"
all=(glibc floor_tls floor_heap cralloc mimalloc mimalloc-sized jemalloc tcmalloc-percpu tcmalloc-thread snmalloc)

features() {
  case "$1" in
    glibc) ;;
    floor_tls | floor_heap) echo "--features $1" ;;
    cralloc) echo "--features asm" ;;
    mimalloc) echo "--features ba_mi" ;;
    mimalloc-sized) echo "--features ba_mi_sized" ;;
    jemalloc) echo "--features ba_je" ;;
    tcmalloc-percpu | tcmalloc-thread) echo "--features ba_tc" ;;
    snmalloc) echo "--features ba_sn" ;;
    *) echo "unknown allocator $1" >&2; exit 2 ;;
  esac
}

# The two tcmalloc modes are one build.
dir() {
  case "$1" in tcmalloc-*) echo "target/tcmalloc${BUILD_TAG:-}" ;; *) echo "target/$1${BUILD_TAG:-}" ;; esac
}

mode=${1:-run}
shift || true
[ $# -gt 0 ] && allocs=("$@") || allocs=("${all[@]}")
common=$(git -C "$here" rev-parse --path-format=absolute --git-common-dir)
export BENCH_BASELINES=${BENCH_BASELINES:-$(dirname "$common")/bench/baselines/out}
page=$(getconf PAGESIZE)
export CFLAGS_aarch64_unknown_linux_gnu=-mcpu=native
export CXXFLAGS_aarch64_unknown_linux_gnu="-mcpu=native -DSNMALLOC_PAGESIZE=$page"

if [ "$mode" = build ]; then
  for a in "${allocs[@]}"; do
    # shellcheck disable=SC2046
    "$scripts/heavy" cargo build -q --profile lto $(features "$a") --target-dir "$(dir "$a")"
  done
  exit 0
fi

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
for _ in $(seq "${RF_ROUNDS:-3}"); do
  for a in "${allocs[@]}"; do
    envs=(RF_SINGLE_ONLY=1)
    [ "$a" = tcmalloc-percpu ] && envs+=(GLIBC_TUNABLES=glibc.pthread.rseq=0)
    MEMLIMIT_QUIET=1 "$scripts/measure" "$scripts/memlimit" -m 8G -t 10m -- \
      env "${envs[@]}" "$(dir "$a")/lto/roofline-rust-alloc" "$a" "${RF_FILTER:-}" >> "$tmp"
  done
done
python3 - "$tmp" "${allocs[@]}" <<'PY'
import collections
import sys
best = collections.defaultdict(lambda: collections.defaultdict(list))
ins = {}
for line in open(sys.argv[1]):
    f = line.split()
    if len(f) < 8 or f[3] != "cyc/op":
        continue
    best[f[1]][f[0]].append(float(f[2]))
    ins[f[1], f[0]] = float(f[4])
allocs = sys.argv[2:]
print(f"{'workload':<14}" + "".join(f"{a:>18}" for a in allocs))
for w in best:
    row = f"{w:<14}"
    for a in allocs:
        v = best[w][a]
        row += f"{min(v):>10.2f} +{max(v) - min(v):<5.2f} " if v else " " * 18
    print(row)
print("instructions")
for w in best:
    print(f"{w:<14}" + "".join(f"{ins.get((w, a), 0):>18.1f}" for a in allocs))
PY
