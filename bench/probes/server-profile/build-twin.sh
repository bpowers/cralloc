#!/usr/bin/env bash
# Builds a line-table twin of a server binary for attribution: the same program, allocator and
# profile as `benchrun build-apps` builds, with `debug = "line-tables-only"` added, into
# bench/target/<BUILD>-dbg/apps/<PROGRAM>. Debug information must not change the code; the
# twin is used only to name the samples of the binary that was measured, after `textcmp.py`
# has shown that their executable sections are byte-identical.
#
#   ../scripts/heavy probes/server-profile/build-twin.sh PROGRAM BUILD FEATURE
#
# Run from bench/. PROGRAM is orders or meilisearch; BUILD the benchrun build name (asmalloc,
# je); FEATURE the bench-alloc feature (asmalloc, je). rpc-server needs no twin: the bench
# workspace's own profile already has `debug = 1`.
#
# With TWIN_PACKAGES="asmalloc bench-alloc", only those packages get line tables, for a program
# built without LTO (Meilisearch), where the rest of the program's code is not rebuilt and the
# allocator's code is compiled in those two crates.
#
# Line tables do change code generation slightly (the order service's .text grew by 7 KB of
# 4 MB), so a twin is not the measured binary: it is measured itself (benchrun's local
# `build = "<BUILD>-dbg"` entries) and attributed within itself.
set -euo pipefail
program="$1" build="$2" feature="$3"
bench="$(cd "$(dirname "$0")/../.." && pwd)"
data="${BENCH_APPS_DATA:-$(git -C "$bench" rev-parse --path-format=absolute --git-common-dir)/../bench/apps/target}"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS
export BENCH_BASELINES="${BENCH_BASELINES:-$(git -C "$bench" rev-parse --path-format=absolute --git-common-dir)/../bench/baselines/out}"
config=()
if [ -n "${TWIN_PACKAGES:-}" ]; then
  for p in $TWIN_PACKAGES; do config+=(--config "profile.release.package.$p.debug=\"line-tables-only\""); done
else
  export CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
fi
cd "$bench/apps/$program"
CARGO_BUILD_BUILD_DIR="$data/build/$program" \
  cargo build --release --locked "${config[@]}" --features "$feature" --target-dir "$bench/target/$build-dbg/apps/$program"
ls -la "$bench/target/$build-dbg/apps/$program/release/$program"
