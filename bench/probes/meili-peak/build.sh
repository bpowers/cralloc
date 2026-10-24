#!/usr/bin/env bash
# Builds a server or app with the allocator at a commit, edited by scripts, as the bench build
# NAME of this checkout (docs/research/meili-peak.md). As meili-tail's build-at.sh: the commit's
# tree is extracted under target/ of this checkout (gitignored; /tmp is memory) and edited; an
# app (bench/apps/PROGRAM) is built with FEATURE as `benchrun build-apps` builds it, sharing the
# main checkout's build directory for the program's crates, into
# bench/target/NAME/apps/PROGRAM/release/PROGRAM; PROGRAM=rpc-server builds the RPC servers of
# the bench workspace into bench/target/NAME/release/rpc-server, as `benchrun build` does.
#
#   [PROGRAM=meilisearch] scripts/heavy bench/probes/meili-peak/build.sh NAME COMMIT FEATURE [EDIT...]
#
# The builds of the note:
#   build.sh mp-main  169b5c7 asmalloc
#   build.sh mp-diag  169b5c7 asmalloc-memstat peak_edit.py proto_edit.py
#   build.sh mp-je    169b5c7 je-stats         peak_edit.py
#   build.sh mp-proto 169b5c7 asmalloc         proto_edit.py
#   PROGRAM=datafusion, orders and rpc-server: build.sh mp-proto 169b5c7 asmalloc proto_edit.py
# (each edit given as bench/probes/meili-peak/NAME.py).
set -euo pipefail
program="${PROGRAM:-meilisearch}"
name="$1" commit="$2" feature="$3"
shift 3
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
shared="$(dirname "$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)")/bench"
tree="$root/target/meili-peak-$name-$program"
rm -rf "$tree"
mkdir -p "$tree"
git -C "$root" archive "$commit" | tar -x -C "$tree"
# The probe's own files come from this checkout, whatever the commit.
mkdir -p "$tree/bench/probes/meili-peak"
cp "$here"/*.rs "$here"/*.py "$tree/bench/probes/meili-peak/"
built="$(git -C "$root" rev-parse "$commit") $feature"
for edit in "$@"; do
  python3 -I "$tree/bench/probes/meili-peak/$(basename "$edit")" "$tree"
  built="$built, $(basename "$edit") $(sha256sum "$edit" | cut -c1-12)"
done
echo "$built" > "$tree/BUILT-FROM"
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS BENCH_LAYOUT_PAD
export BENCH_BASELINES="$shared/baselines/out"
if [ "$program" = rpc-server ]; then
  (cd "$tree/bench" && cargo build --release -p bench-rpc --features "bench-alloc/$feature" \
    --target-dir "$root/bench/target/$name")
else
  target="$root/bench/target/$name/apps/$program"
  mkdir -p "$target"
  (cd "$tree/bench/apps/$program" && CARGO_BUILD_BUILD_DIR="$shared/apps/target/build/$program" \
    cargo build --release --locked --features "$feature" --target-dir "$target")
fi
cp "$tree/BUILT-FROM" "$root/bench/target/$name/BUILT-FROM"
rm -rf "$tree"
echo "$name $program: $(cat "$root/bench/target/$name/BUILT-FROM")"
