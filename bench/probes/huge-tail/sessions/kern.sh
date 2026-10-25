#!/usr/bin/env bash
# Interleaved runs of one workload with the switch off and on (one binary) and with the
# incumbents, on the suite's measurement CPUs, with page walks by size counted and the
# kernel's huge-page fault counts around each run. From the checkout:
#   scripts/measure --both-l3 bench/probes/huge-tail/sessions/kern.sh OUTDIR ROUNDS "ARMS" WORKLOAD ARGS...
# ARMS: name=BUILD:ASMALLOC_HT[:VAR=value], e.g. "off=ht2:t0 on=ht2:t1 je=je:- nobg=ht2:t1:ASMALLOC_BACKGROUND=0".
set -euo pipefail
out="$1" rounds="$2" arms="$3"
shift 3
cpus="${CPUS:-0-3,8-11}"
mkdir -p "$out"
export BENCH_EVENTS=dtlb_miss=0xff45,walk_4k=0x1045,walk_2m=0x4045
vm() { echo "# $1 free2m=$(bench/probes/huge-tail/free2m.sh) $(grep -E '^thp_fault_alloc |^thp_fault_fallback ' /proc/vmstat | tr ' \n' '= ')"; }
for r in $(seq 1 "$rounds"); do
  for arm in $(printf '%s\n' $arms | shuf); do
    name="${arm%%=*}" rest="${arm#*=}"
    build="${rest%%:*}" ht="${rest#*:}" extra=HT_ARM=1
    case "$ht" in *:*) extra="${ht#*:}" ht="${ht%%:*}" ;; esac
    file="$out/$name.r$r.txt"
    echo "# $(date -Is) arm=$arm cpus=$cpus args: $*" > "$file"
    vm before >> "$file"
    env MEMLIMIT_QUIET=1 BENCH_CPUS="$cpus" ASMALLOC_HT="$ht" HT_DIAG="$PWD/$file.ht" "$extra" scripts/memlimit -m 12G -t 20m -- \
      bash -c '
        cg=/sys/fs/cgroup$(sed -n "s/^0:://p" /proc/self/cgroup)
        st() { echo "# $1 cg_thp_fault_alloc=$(awk "/^thp_fault_alloc /{print \$2}" $cg/memory.stat) cg_thp_collapse_alloc=$(awk "/^thp_collapse_alloc /{print \$2}" $cg/memory.stat)"; }
        st cgbefore; "$@"; s=$?; st cgafter; exit $s
      ' kern taskset -c "$cpus" "bench/target/$build/release/workload" "$@" >> "$file" 2>&1 || echo "# exit $?" >> "$file"
    vm after >> "$file"
  done
done
