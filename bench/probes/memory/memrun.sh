#!/usr/bin/env bash
# usage: memrun.sh <worktree> <label,label,...> <cpus> <tag> <workload args...>
# Runs one memory-shape workload under each build, capped, in a cgroup of its own, and prints
# its MEM lines in MiB (every EVERY-th "after" line, default 4: one a second) and the cgroup
# peak. CONFINE=1 also confines the process to <cpus> with taskset, as the benchmark runner does.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$1"; feats="$2"; cpus="$3"; tag="$4"; shift 4
out="${OUT:-$root/bench/target/memory-out}"
mkdir -p "$out"
for feat in ${feats//,/ }; do
  bin="$root/bench/target/$feat/release/workload"
  log="$out/$tag.$feat.log"
  stat="/dev/null"
  case "$feat" in *-ms|cralloc-memstat) stat="$out/$tag.$feat.memstat" ;; esac
  rm -f "$log.peak"
  wrap=()
  if [ -n "${CONFINE:-}" ]; then wrap=(taskset -c "$cpus"); fi
  CRALLOC_MEMSTAT="$stat" CRALLOC_MEMSTAT_MS="${MS:-10}" BENCH_CPUS="$cpus" MEMLIMIT_QUIET=1 \
    "$root/scripts/memlimit" -m "${CAP:-4G}" -t 5m -- \
    bash "$here/incg.sh" "$log.peak" "${wrap[@]}" "$bin" "$@" >"$log" 2>&1
  echo "== $tag $feat  cgroup_peak_MiB=$(awk '{printf "%.1f", $1/1048576}' "$log.peak" 2>/dev/null)"
  grep -a '^MEM' "$log" | awk -v every="${EVERY:-4}" '
    { for (i = 2; i <= NF; i++) { split($i, kv, "="); v[kv[1]] = kv[2] } }
    v["phase"] == "after" { n++; if (n % every != 0) next }
    { printf "  %-8s t=%6dms rss_anon=%8.1f pte=%5.2f thp=%7.1f\n", v["phase"], v["t_ms"], v["rss_anon"]/1048576, v["pte"]/1048576, v["thp"]/1048576 }'
  grep -a -v '^MEM\|BENCH_RESULT' "$log" | head -5
done
