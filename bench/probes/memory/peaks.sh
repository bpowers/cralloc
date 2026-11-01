#!/usr/bin/env bash
# usage: peaks.sh <worktree> <label,label,...> <job>...
# Runs each named job under each build (bench/target/<label>), capped, each process in a
# cgroup of its own, and prints that cgroup's memory.peak in MiB. Threads are pinned by the
# workload (BENCH_CPUS); the process is not confined. Builds whose label ends in -ms or is
# cralloc-memstat write their breakdown to $OUT/<job>.<label>.memstat.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$1"; feats="$2"; shift 2
out="${OUT:-$root/bench/target/memory-out}"
mkdir -p "$out"

run_one() {
  local feat="$1" cpus="$2" tag="$3"; shift 3
  local bin="$root/bench/target/$feat/release/workload"
  local log="$out/$tag.$feat.log"
  local stat="/dev/null"
  case "$feat" in *-ms|cralloc-memstat) stat="$out/$tag.$feat.memstat" ;; esac
  rm -f "$log.peak"
  CRALLOC_MEMSTAT="$stat" BENCH_CPUS="$cpus" MEMLIMIT_QUIET=1 "$root/scripts/memlimit" -m "${CAP:-4G}" -t 5m -- \
    bash "$here/incg.sh" "$log.peak" "$bin" "$@" >"$log" 2>&1
  local peak hwm prim
  peak=$(awk '{printf "%.1f", $1/1048576}' "$log.peak" 2>/dev/null)
  hwm=$(grep -o '"vm_hwm":[0-9]*' "$log" | tail -1 | awk -F: '{printf "%.1f", $2/1048576}')
  prim=$(grep -o '"primary":{[^}]*}' "$log" | tail -1 | grep -o '"value":[0-9.e+-]*' | cut -d: -f2)
  printf '%-10s %-18s cgroup_peak_MiB=%-7s vm_hwm_MiB=%-7s primary=%s\n' "$tag" "$feat" "$peak" "$hwm" "$prim"
}

for job in "$@"; do
  case "$job" in
    pipe2)    cpus=0,1;     args="pipeline threads=2 sizes=small objects=2M" ;;
    pipe2big) cpus=0,1;     args="pipeline threads=2 sizes=small objects=10M" ;;
    pipemix)  cpus=0,1;     args="pipeline threads=2 sizes=mixed objects=2M" ;;
    pipe4)    cpus=0,1,2,3; args="pipeline threads=4 sizes=small objects=2M" ;;
    exch2)    cpus=0,8;     args="exchange threads=2 sizes=small objects=2M" ;;
    mpmc)     cpus=0,1,2,3; args="mpmc producers=3 consumers=1 sizes=small objects=2M" ;;
    forkjoin) cpus=0,1,2,3; args="forkjoin threads=4 sizes=small reps=10" ;;
    forkjoin8) cpus=0,1,2,3,8,9,10,11; args="forkjoin threads=8 sizes=small reps=10" ;;
    spawn)    cpus=0,1,2,3; args="spawn threads=4 rounds=4000 sizes=small" ;;
    churn1k)  cpus=2;       args="churn live=1k ops=5M sizes=small" ;;
    churn1M)  cpus=2;       args="churn live=1M ops=5M sizes=small" ;;
    churn1Mfull) cpus=2;  args="churn live=1M ops=20M sizes=small" ;;
    churnmixfull) cpus=2; args="churn live=100k ops=20M sizes=mixed" ;;
    forkjoinfull) cpus=0,1,2,3; args="forkjoin threads=4 sizes=small" ;;
    churnmix) cpus=2;       args="churn live=100k ops=5M sizes=mixed" ;;
    btree)    cpus=2;       args="map kind=btree n=200k reps=2 idle_ms=200" ;;
    hmap)     cpus=2;      args="map kind=hash n=200k reps=2 idle_ms=200" ;;
    tree)     cpus=2;       args="tree depth=18 reps=3" ;;
    vecgrow)  cpus=2;       args="vecgrow vecs=1 max=64Mi reps=2" ;;
    vecgrow64) cpus=2;      args="vecgrow vecs=64 max=4Mi reps=2" ;;
    json)     cpus=2;       args="json size=8Mi reps=2" ;;
    *) echo "unknown job $job"; continue ;;
  esac
  for feat in ${feats//,/ }; do
    # shellcheck disable=SC2086
    run_one "$feat" "$cpus" "$job" $args
  done
done
