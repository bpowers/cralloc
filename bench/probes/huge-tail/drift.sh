#!/usr/bin/env bash
# Runs heap-drift's drift workload with one build under a memory cap, as heap-drift's run.sh
# does, and records around it the cgroup's huge-page counters and the kernel's (global)
# fallback counts, so that each run says whether it got huge pages. For
# docs/research/huge-tail.md.
#
#   scripts/heavy bench/probes/huge-tail/drift.sh BUILD OUTFILE [VAR=value ...] -- [key=value ...]
#
# BUILD is a directory under bench/target (ht1, ht1-memstat, je, glibc). VAR=value pairs before
# `--` go into the program's environment (ASMALLOC_HT=..., HT_DIAG is set to OUTFILE.ht). A
# memstat build writes its breakdowns to OUTFILE.memstat.
#   DRIFT_MEM      the cap, default 8G (never above 16G)
#   DRIFT_TIMEOUT  default 90m
#   DRIFT_CPUS     a CPU list to run on (taskset), default: wherever the caller runs
set -euo pipefail
build="$1" out="$2"
shift 2
env=()
while [ $# -gt 0 ] && [ "$1" != -- ]; do
  env+=("$1")
  shift
done
[ "${1:-}" = -- ] && shift
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
exe="$root/bench/target/$build/release/workload"
[ -x "$exe" ] || { echo "no workload binary at $exe" >&2; exit 2; }
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
case "$build" in
  *-memstat) env+=(ASMALLOC_MEMSTAT="$out.memstat" ASMALLOC_MEMSTAT_MS=0 ASMALLOC_MEMSTAT_CPU=15) ;;
esac
env+=(HT_DIAG="$out.ht")
mkdir -p "$(dirname "$out")"
rm -f "$out.ht"
{
  echo "# $(date -Is) build=$build exe=$(readlink -f "$exe") built-from=$(cat "$root/bench/target/$build/BUILT-FROM" 2>/dev/null)"
  echo "# env: ${env[*]}"
  echo "# args: $*"
} > "$out"
pin=()
[ -n "${DRIFT_CPUS:-}" ] && pin=(taskset -c "$DRIFT_CPUS")
status=0
MEMLIMIT_QUIET=0 "$root/scripts/memlimit" -m "${DRIFT_MEM:-8G}" -t "${DRIFT_TIMEOUT:-90m}" -- \
  bash -c '
    cg=/sys/fs/cgroup$(sed -n "s/^0:://p" /proc/self/cgroup)
    stat() { echo "# $1 free2m=$(awk "{ n = 0; for (i = 14; i <= NF; i++) n += \$i * 2 ^ (i - 14); t += n } END { print t }" /proc/buddyinfo) cg_thp_fault_alloc=$(awk "/^thp_fault_alloc /{print \$2}" $cg/memory.stat) cg_thp_collapse_alloc=$(awk "/^thp_collapse_alloc /{print \$2}" $cg/memory.stat) vm_thp_fault_alloc=$(awk "/^thp_fault_alloc /{print \$2}" /proc/vmstat) vm_thp_fault_fallback=$(awk "/^thp_fault_fallback /{print \$2}" /proc/vmstat) vm_thp_split_page=$(awk "/^thp_split_page /{print \$2}" /proc/vmstat) peak=$(cat $cg/memory.peak 2>/dev/null)"; }
    stat before
    "$@" && s=0 || s=$?
    stat after
    exit $s
  ' drift env "${env[@]}" "${pin[@]}" "$exe" drift "$@" >> "$out" 2>&1 || status=$?
echo "# exit $status $(date -Is)" >> "$out"
exit "$status"
