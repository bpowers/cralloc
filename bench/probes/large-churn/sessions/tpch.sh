#!/usr/bin/env bash
# tpchgen under the switch's arms and glibc. From the checkout:
#   scripts/measure --both-l3 bench/probes/large-churn/sessions/tpch.sh OUT.jsonl [ROUNDS]
# RUST_SCREEN_ROOT names the screening's tree (bench/probes/rust-screen); the default is the
# one the grown-slots study used.
set -euo pipefail
lib="$(pwd)/bench/target/lcs/libasmalloc_preload.so"
export RUST_SCREEN_ROOT="${RUST_SCREEN_ROOT:-/home/bpowers/src/asmalloc/.claude/worktrees/agent-ab8583755fe797b69/target/rust-screen}"
export APPF_OUT="$(pwd)/target/large-churn/appf"
mkdir -p "$APPF_OUT"
exec python3 -I bench/probes/large-churn/tpch.py "$1" --rounds "${2:-3}" \
  --arms "main=$lib:t0p0b0000a0,prev=$lib:t0p1b0000a0,age=$lib:t0p0b0000a1,prevage=$lib:t0p1b0000a1,glibc=-"
