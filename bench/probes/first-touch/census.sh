#!/usr/bin/env bash
# A census of every call that changes or reads the address space's layout, the server's own
# as well as the allocator's: one session.sh run, with strace attached to the server for DUR
# seconds from AT seconds after the load generator starts. strace stops every thread at every
# system call, so the run's timings are not measurements; the counts are what it is for. Run
# from bench/ of this checkout, under scripts/measure --both-l3:
#
#   ../scripts/measure --both-l3 probes/first-touch/census.sh OUTDIR TAG SUITE JOB MEMCAP [AT [DUR]]
set -euo pipefail
out="$1" tag="$2" suite="$3" job="$4" cap="$5" at="${6:-10}" dur="${7:-8}"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
gen_bin="$bench/target/loadgen/release/loadgen"
before=$(pgrep -f "$gen_bin" || true)
"$here/session.sh" "$out" "$tag" "$suite" "$job" "$cap" &
sess=$!
pid=""
for _ in $(seq 1 3000); do
  [ -f "$out/$tag.wl/procs" ] && pid=$(awk 'NR == 1 { print $1 }' "$out/$tag.wl/procs") && [ -n "$pid" ] && break
  sleep 0.1
done
gen=""
for _ in $(seq 1 6000); do
  gen=$(pgrep -n -f "$gen_bin" || true)
  [ -n "$gen" ] && [ "$gen" != "$before" ] && break
  gen=""
  sleep 0.05
done
if [ -n "$pid" ] && [ -n "$gen" ]; then
  sleep "$at"
  echo "strace $(date -Is) pid $pid at $at for $dur" > "$out/$tag.strace.info"
  taskset -c 14 timeout -s INT "$dur" strace -f -p "$pid" -T -ttt -o "$out/$tag.strace" \
    -e trace=mmap,munmap,mprotect,madvise,mremap,brk,mincore,mlock,munlock,msync || true
fi
wait $sess
