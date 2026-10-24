#!/usr/bin/env bash
# One benchrun session with the patched runner (runner_edit.py: raw dTLB events in every
# process's counters and each server group's), the allocators given, in random order. From
# the checkout, under scripts/measure --both-l3:
#   scripts/measure --both-l3 bench/probes/whole-block-huge/sessions/run.sh OUT.jsonl SUITE ONLY ALLOCATORS [ROUNDS] [MEMCAP]
# SUITE and ONLY as benchrun takes them (`--only "=connect log grpc h2"`); ALLOCATORS are the
# local entries of bench/allocators.toml (wbh-main, wbh-all2, wbh-nz2, wbh-all16, mi-t, tc-t).
set -euo pipefail
out="$(pwd)/$1" suite="$2" only="$3" allocs="$4" rounds="${5:-1}" cap="${6:-4G}"
export WBH_BENCH_DIR="$(pwd)/bench"
# A batch app counts the events itself (BENCH_EVENTS in its entry); counting them twice
# would multiplex the counters.
[ -n "${NOPROC:-}" ] || export WBH_PROC_EVENTS="dtlb_miss=0xff45,walk_4k=0x1045,walk_2m=0x4045"
cd bench
exec target/wbh/benchrun run --suite "$suite" --only "$only" --allocators "$allocs" \
  --rounds "$rounds" --mem-cap "$cap" --out "$out"
