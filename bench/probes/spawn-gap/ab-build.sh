#!/usr/bin/env bash
# usage: ab-build.sh [--trace] NAME=COMMIT...
# Builds each commit, from `git archive` into target/src-NAME (target/trc-NAME with --trace,
# the diagnostics patch applied), as benchrun's `build = "asm-NAME"` entries expect: the
# workloads and rpc-server into bench/target/asm-NAME, and the order service and boa into
# bench/target/asm-NAME/apps; with --trace, the workloads and rpc-server with bench-alloc's
# `trace` feature into bench/target/trc-NAME. Keep NAMEs one length (the binaries' paths).
# Run it under scripts/heavy.
set -euo pipefail
W=$(cd "$(dirname "$0")/../../.." && pwd)
trace=
if [ "${1:-}" = "--trace" ]; then trace=1; shift; fi
export BENCH_BASELINES="${BENCH_BASELINES:-$(git -C "$W" rev-parse --path-format=absolute --git-common-dir)/../bench/baselines/out}"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
for spec in "$@"; do
  name=${spec%%=*}
  commit=${spec#*=}
  if [ -n "$trace" ]; then src="$W/target/trc-$name"; else src="$W/target/src-$name"; fi
  rm -rf "$src" && mkdir -p "$src"
  git -C "$W" archive --format=tar "$commit" | tar -x -C "$src"
  if [ -n "$trace" ]; then
    (cd "$src" && patch -p1 -s < "$W/bench/probes/spawn-gap/diagnostics.patch")
    (cd "$src/bench" && cargo build --release -p bench-workloads -p bench-rpc \
       --features bench-alloc/asmalloc,bench-alloc/trace --target-dir "$W/bench/target/trc-$name")
    continue
  fi
  (cd "$src/bench" && cargo build --release -p bench-workloads -p bench-rpc \
     --features bench-alloc/asmalloc --target-dir "$W/bench/target/asm-$name")
  for app in orders boa; do
    (cd "$src/bench/apps/$app" && CARGO_BUILD_BUILD_DIR="$W/target/appbuild-$name/$app" \
       cargo build --release --locked --features asmalloc \
       --target-dir "$W/bench/target/asm-$name/apps/$app")
  done
done
