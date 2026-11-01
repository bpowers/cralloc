#!/usr/bin/env bash
# One run for acct.sh, inside a capped scope (scripts/memlimit): moves the scope's processes to a
# leaf, runs the measured program in a fresh child cgroup confined to its CPUs, runs the RPC
# client against it if the job has one, and writes the child cgroup's memory.peak.
# usage: acct-one.sh <out prefix> <cpus> <bin dir> <client> <program args> [<client args>]
set -uo pipefail
prefix="$1"; cpus="$2"; bin="$3"; client="$4"; cmd="$5"; clientargs="${6:-}"
cg="/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)"
mkdir -p "$cg/sup" "$cg/run"
for pid in $(cat "$cg/cgroup.procs"); do echo "$pid" >"$cg/sup/cgroup.procs" 2>/dev/null || true; done
echo +memory >"$cg/cgroup.subtree_control"
read -r -a argv <<<"$cmd"
argv[0]="$bin/${argv[0]}"
export BENCH_CPUS="$cpus" BENCH_MEMACCT="$prefix.acct" CRALLOC_MEMSTAT="$prefix.memstat" CRALLOC_MEMSTAT_CPU=15 \
  BENCH_MEMACCT_WAIT_MS=2500
if [ -z "$clientargs" ]; then
  bash -c 'echo $$ >"$0/run/cgroup.procs"; exec taskset -c "$1" "${@:2}"' "$cg" "$cpus" "${argv[@]}"
else
  fifo="$prefix.fifo"
  rm -f "$fifo"; mkfifo "$fifo"
  bash -c 'echo $$ >"$0/run/cgroup.procs"; exec taskset -c "$1" "${@:2}"' "$cg" "$cpus" "${argv[@]}" >"$fifo" &
  server=$!
  addr=""
  exec 3<"$fifo"
  while read -r line <&3; do
    echo "$line"
    case "$line" in BENCH_LISTEN*) addr="${line#BENCH_LISTEN }"; break ;; esac
  done
  read -r -a cargv <<<"$clientargs"
  BENCH_CPUS=8,9,10,11 taskset -c 8,9,10,11 env -u BENCH_MEMACCT -u CRALLOC_MEMSTAT \
    "$client" "${cargv[0]}" "addr=$addr" "${cargv[@]:1}" >"$prefix.client" 2>&1
  kill -TERM "$server"
  cat <&3
  wait "$server"
  rm -f "$fifo"
fi
cat "$cg/run/memory.peak" >"$prefix.peak"
