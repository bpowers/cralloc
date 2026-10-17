#!/usr/bin/env bash
# The worst-case workloads of docs/research/memory.md, each under every named build.
# usage: worst.sh <worktree> <label,label,...> [case...]
# Cases: burst-idle, phase, cpus-few-classes, classes-few-cpus, threads-few-cpus, sparse, tiny.
# Prints, per build, anonymous resident MiB at each phase and every four seconds after.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$1"; builds="$2"; shift 2
[ $# -gt 0 ] || set -- burst-idle phase cpus-few-classes classes-few-cpus threads-few-cpus sparse tiny
eight=0,1,2,3,8,9,10,11
run() { EVERY=16 bash "$here/memrun.sh" "$root" "$builds" "$@" 2>&1 | grep -v ' start \| end '; }
for case in "$@"; do
  echo "#### $case"
  case "$case" in
    # Free 218 MiB in random order, then 2000 64-byte objects every millisecond.
    burst-idle) run 2 burst-idle quiet live=2M sizes=small secs=12 after=burst after_sizes=s64 ;;
    # Eight CPUs use every small class; then seven block and one keeps to one class.
    phase) run "$eight" phase touch threads=8 secs=8 sizes=s64 ;;
    cpus-few-classes) run "$eight" cpus-few-classes touch threads=8 classes=4 secs=8 sizes=s64 ;;
    classes-few-cpus) run 0,1 classes-few-cpus touch threads=2 secs=8 sizes=s64 ;;
    # Sixteen threads on two CPUs: per-thread caches against per-CPU ones.
    threads-few-cpus) run 0,1 threads-few-cpus touch threads=16 secs=8 sizes=s64 ;;
    sparse)
      run 2 sparse-1m sparse n=256 size=1Mi touch=4096
      run 2 sparse-64k sparse n=4096 size=64Ki touch=4096 ;;
    tiny) run 2 tiny tiny ;;
    *) echo "unknown case $case" ;;
  esac
done
