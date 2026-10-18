#!/usr/bin/env bash
# Run a command in a fresh child cgroup of the current (delegated, memory-capped) scope and
# write that cgroup's memory.peak to a file, so the number is the command's alone.
# usage: incg.sh <peak file> <command> [args...]     (run under scripts/memlimit)
set -uo pipefail
peakfile="$1"; shift
cg="/sys/fs/cgroup$(cut -d: -f3 /proc/self/cgroup)"
mkdir -p "$cg/sup" "$cg/run"
# Every process of the scope must sit in a leaf before the scope can hand the memory
# controller to its children.
for pid in $(cat "$cg/cgroup.procs"); do echo "$pid" >"$cg/sup/cgroup.procs" 2>/dev/null || true; done
echo +memory >"$cg/cgroup.subtree_control"
bash -c 'echo $$ >"$0/run/cgroup.procs"; exec "$@"' "$cg" "$@"
status=$?
cat "$cg/run/memory.peak" >"$peakfile"
exit $status
