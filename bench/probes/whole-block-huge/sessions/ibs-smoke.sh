#!/usr/bin/env bash
# Whether IBS op sampling gives data addresses and dTLB outcomes for a workload, as the
# attribution of walks to classes needs. A check, under scripts/heavy:
#   scripts/heavy bench/probes/whole-block-huge/sessions/ibs-smoke.sh OUTDIR
set -euo pipefail
out="$1"
mkdir -p "$out"
B="$(pwd)/bench/target/wbh"
ASMALLOC_WBH=m0f002d1 WBH_DIAG="$out/map.diag" BENCH_CPUS=12 scripts/memlimit -m 3G -t 3m -- \
  perf record -e ibs_op/cnt_ctl=1/ -c 100003 -d -o "$out/perf.data" -- "$B/release/workload" map kind=hash n=1M reps=1 > /dev/null
perf script -i "$out/perf.data" -F ip,addr,data_src,weight 2>/dev/null | head -20
perf report -i "$out/perf.data" --stdio --mem-mode --sort=tlb 2>/dev/null | head -30
