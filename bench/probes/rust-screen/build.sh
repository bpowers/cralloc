#!/usr/bin/env bash
# Builds one candidate from crates.io at a pinned version with the system allocator, records
# how long the build took, and removes the build tree.
#
# Usage: build.sh NAME CRATE VERSION [cargo install args...]
#   Run under scripts/heavy. Binaries land in ROOT/bin/NAME/, the record in ROOT/builds.jsonl.
#   Pass --no-default-features and the features to keep when the crate's defaults pick an
#   allocator of its own. BUILD_UNLOCKED=1 resolves dependencies afresh instead of using the
#   crate's published lockfile, for crates whose lockfile no longer builds.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="${RUST_SCREEN_ROOT:-$here/../../../target/rust-screen}"
root="$(cd "$root" && pwd)"
name="$1" crate="$2" version="$3"
shift 3
mkdir -p "$root/bin/$name" "$root/build"
build="$root/build/$name"
start=$(date +%s)
status=0
locked=(--locked)
[ -n "${BUILD_UNLOCKED:-}" ] && locked=()
CARGO_TARGET_DIR="$build" cargo install "${locked[@]}" --force --root "$root/inst/$name" \
  --version "=$version" "$crate" "$@" >"$root/build/$name.log" 2>&1 || status=$?
end=$(date +%s)
cp -f "$root/inst/$name/bin/"* "$root/bin/$name/" 2>/dev/null || true
rm -rf "$build" "$root/inst/$name"
printf '{"name":"%s","crate":"%s","version":"%s","args":"%s","status":%d,"build_s":%d}\n' \
  "$name" "$crate" "$version" "$*" "$status" $((end - start)) >>"$root/builds.jsonl"
echo "$name: status $status in $((end - start)) s"
tail -3 "$root/build/$name.log"
