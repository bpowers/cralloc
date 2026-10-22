#!/usr/bin/env bash
# Builds the three servers of the servers-thread suite into bench/target/NAME with bench-alloc's
# FEATURE, as `benchrun build` and `build-apps` would (the shared build directory of each
# program, its own profile), for builds that benchrun leaves to hand: the scratch diagnostic
# builds made after diag_edit.py. Run from bench/ under scripts/heavy:
#
#   ../scripts/heavy probes/default-thread/build.sh NAME FEATURE
set -euo pipefail
name="$1" feature="$2"
bench="$(cd "$(dirname "$0")/../.." && pwd)"
data="${BENCH_APPS_DATA:-/home/bpowers/src/asmalloc/bench/apps/target}"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
export BENCH_BASELINES="${BENCH_BASELINES:-/home/bpowers/src/asmalloc/bench/baselines/out}"
(cd "$bench" && cargo build --release -p bench-rpc --features "bench-alloc/$feature" --target-dir "target/$name")
for program in orders meilisearch; do
  (cd "$bench/apps/$program" && CARGO_BUILD_BUILD_DIR="$data/build/$program" \
    cargo build --release --locked --features "bench-alloc/$feature" --target-dir "$bench/target/$name/apps/$program")
done
ls -la "$bench/target/$name/release/rpc-server" "$bench/target/$name/apps/orders/release/orders" \
  "$bench/target/$name/apps/meilisearch/release/meilisearch"
