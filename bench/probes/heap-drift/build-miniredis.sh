#!/usr/bin/env bash
# Builds mini-redis (bench/apps/miniredis) of this checkout once per feature, as `benchrun
# build-apps` does, into bench/target/NAME/apps/miniredis, sharing the main checkout's build
# directory for its dependencies; and the load generator, with glibc, into
# bench/target/loadgen. Run under scripts/heavy:
#
#   scripts/heavy bench/probes/heap-drift/build-miniredis.sh [NAME...]
#
# NAME is glibc or a feature of bench/apps/miniredis/Cargo.toml (je mi tc asmalloc live-bytes
# asmalloc-memstat); the default is all of those.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
shared="$(dirname "$(git -C "$bench" rev-parse --path-format=absolute --git-common-dir)")/bench"
export BENCH_BASELINES="${BENCH_BASELINES:-$shared/baselines/out}"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS BENCH_LAYOUT_PAD
names=("$@")
[ ${#names[@]} -gt 0 ] || names=(glibc je mi tc asmalloc live-bytes asmalloc-memstat)
for name in "${names[@]}"; do
  features=()
  [ "$name" = glibc ] || features=(--features "$name")
  echo "== miniredis $name"
  (cd "$bench/apps/miniredis" && CARGO_BUILD_BUILD_DIR="$shared/apps/target/build/miniredis" \
    cargo build --release --locked "${features[@]}" --target-dir "$bench/target/$name/apps/miniredis")
done
echo "== loadgen"
(cd "$bench/probes/loadgen" && cargo build --release --locked --target-dir "$bench/target/loadgen")
