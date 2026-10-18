#!/usr/bin/env bash
# The small-heap jobs of docs/research/memory.md ("Small heaps"), each run as the full suite runs
# it (same arguments, CPUs and confinement), capped, in a cgroup of its own.
#
# usage: acct.sh <worktree> <label,label,...> <rounds> <job>...
# Jobs: pipe2 pipe2free pipemix mpmc44 mpmc31 spawn4 churn100 churn1k tiny echo64 log64 log256, and
# for timing on CPUs 0-7 only: exch2 pipex2 mpmc44a churn100k churn1M.
#
# Per job, round and build (bench/target/<label>/release), interleaved by round: the cgroup's
# memory.peak, the workload's primary metric, and in $OUT (default bench/target/memory-out/acct)
# the accounting files a build with `memacct` writes at exit (<tag>.acct) and, for
# asmalloc-memstat builds, the heap breakdown (<tag>.memstat). acct.py reads them.
#
# Hold /tmp/asmalloc-measure.lock around it: the jobs run on the measurement CPUs.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$1"; labels="$2"; rounds="$3"; shift 3
out="${OUT:-$root/bench/target/memory-out/acct}"
mkdir -p "$out"
tgt="$root/bench/target"
client="${CLIENT:-$tgt/sys-acct/release/rpc-client}"

job_args() {
  case "$1" in
    pipe2)     echo "0,1|workload pipeline threads=2 sizes=small prime=yes" ;;
    pipe2free) echo "0,1|workload pipeline threads=2 sizes=small prime=no" ;;
    pipemix)   echo "0,1|workload pipeline threads=2 sizes=mixed prime=yes" ;;
    mpmc44)    echo "0,1,2,3,8,9,10,11|workload mpmc producers=4 consumers=4 sizes=small prime=yes" ;;
    mpmc31)    echo "0,1,2,3|workload mpmc producers=3 consumers=1 sizes=small prime=yes" ;;
    spawn4)    echo "0,1,2,3|workload spawn threads=4 rounds=20000 sizes=small" ;;
    # For timing on one L3 domain (CPUs 0-7, the machine's measurement CPUs).
    exch2)     echo "0,1|workload exchange threads=2 sizes=small" ;;
    pipex2)    echo "0,1,2,3|workload pipeline threads=2 chains=2 sizes=small prime=yes" ;;
    mpmc44a)   echo "0,1,2,3,4,5,6,7|workload mpmc producers=4 consumers=4 sizes=small prime=yes" ;;
    churn100k) echo "2|workload churn live=100k ops=20M sizes=small" ;;
    churn1M)   echo "2|workload churn live=1M ops=20M sizes=small" ;;
    churn100)  echo "2|workload churn live=100 ops=20M sizes=small" ;;
    churn1k)   echo "2|workload churn live=1k ops=20M sizes=small" ;;
    tiny)      echo "2|workload tiny" ;;
    echo64)    echo "0,1,2,3|rpc-server echo|echo concurrency=64 warmup_ms=2000 measure_ms=5000" ;;
    log64)     echo "0,1,2,3|rpc-server log|log concurrency=64 conns=8 warmup_ms=2000 measure_ms=5000" ;;
    log256)    echo "0,1,2,3|rpc-server log|log concurrency=256 conns=8 warmup_ms=2000 measure_ms=5000" ;;
    *) return 1 ;;
  esac
}

for round in $(seq 1 "$rounds"); do
  for job in "$@"; do
    spec="$(job_args "$job")" || { echo "unknown job $job"; continue; }
    IFS='|' read -r cpus cmd clientargs <<<"$spec"
    # A new random order of builds for every job and round.
    for label in $(tr ',' '\n' <<<"$labels" | shuf); do
      tag="$job.$label.$round"
      rm -f "$out/$tag".*
      MEMLIMIT_QUIET=1 "$root/scripts/memlimit" -m "${CAP:-4G}" -t 5m -- \
        bash "$here/acct-one.sh" "$out/$tag" "$cpus" "$tgt/$label/release" "$client" "$cmd" "$clientargs" \
        >"$out/$tag.log" 2>&1
      peak=$(awk '{printf "%.3f", $1/1048576}' "$out/$tag.peak" 2>/dev/null)
      prim=$(grep -o '"primary":{[^}]*}' "$out/$tag.log" | tail -1 | grep -o '"value":[0-9.e+-]*' | cut -d: -f2)
      printf '%-10s %-14s round=%s cgroup_peak_MiB=%-8s primary=%s\n' "$job" "$label" "$round" "$peak" "$prim"
    done
  done
done
