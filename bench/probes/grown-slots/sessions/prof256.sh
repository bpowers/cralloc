#!/usr/bin/env bash
# Where the 256 MiB growth job spends its user cycles, main against the product build: a
# perf profile of each, user cycles only, on CPU 2. From the checkout:
#   scripts/measure bench/probes/grown-slots/sessions/prof256.sh OUTDIR
set -euo pipefail
out="$1"
mkdir -p "$out"
for build in ocm oc; do
  BENCH_CPUS=2 scripts/memlimit -m 4G -- taskset -c 2 perf record -q -e cycles:u -o "$out/$build.data" \
    bench/target/$build/release/workload vecgrow vecs=1 max=256Mi reps=4 > "$out/$build.out" 2>&1
  perf report -q -i "$out/$build.data" --no-children --sort sym --stdio 2>/dev/null | head -15 > "$out/$build.txt"
  BENCH_CPUS=2 scripts/memlimit -m 4G -- taskset -c 2 perf stat -x, -e cycles:u,cycles:k,page-faults,LLC-load-misses,LLC-store-misses \
    bench/target/$build/release/workload vecgrow vecs=1 max=256Mi reps=4 > /dev/null 2> "$out/$build.stat"
done
