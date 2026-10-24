#!/usr/bin/env bash
# Builds Meilisearch with the allocator at a commit, edited by scripts, as the bench build NAME
# of this checkout (docs/research/meili-peak.md). As meili-tail's build-at.sh: the commit's tree
# is extracted under target/ of this checkout (gitignored; /tmp is memory), edited, and its
# bench/apps/meilisearch built with FEATURE as `benchrun build-apps` builds it, sharing the main
# checkout's build directory for the program's crates; the binary lands in
# bench/target/NAME/apps/meilisearch/release/meilisearch of this checkout.
#
#   scripts/heavy bench/probes/meili-peak/build.sh NAME COMMIT FEATURE [EDIT-SCRIPT...]
#
# The builds of the note:
#   build.sh mp-main  169b5c7 asmalloc
#   build.sh mp-diag  169b5c7 asmalloc-memstat bench/probes/meili-peak/peak_edit.py
#   build.sh mp-je    169b5c7 je-stats         bench/probes/meili-peak/peak_edit.py
#   build.sh mp-proto 169b5c7 asmalloc-memstat bench/probes/meili-peak/peak_edit.py \
#       bench/probes/meili-peak/proto_edit.py
set -euo pipefail
name="$1" commit="$2" feature="$3"
shift 3
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
shared="$(dirname "$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)")/bench"
tree="$root/target/meili-peak-$name"
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
target="$root/bench/target/$name/apps/meilisearch"
mkdir -p "$target"
cd "$tree/bench/apps/meilisearch"
env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS -u BENCH_LAYOUT_PAD \
  CARGO_BUILD_BUILD_DIR="$shared/apps/target/build/meilisearch" BENCH_BASELINES="$shared/baselines/out" \
  cargo build --release --locked --features "$feature" --target-dir "$target"
cp "$tree/BUILT-FROM" "$root/bench/target/$name/BUILT-FROM"
rm -rf "$tree"
echo "$name: $(cat "$root/bench/target/$name/BUILT-FROM")"
