#!/usr/bin/env bash
# variant.sh TAG [--diag] [EDIT.py...]: copies the worktree's sources to $APPF_OUT/var/TAG,
# applies the diagnostic patch (with --diag) and runs each edit script in the copy (edits/ has
# one per experiment), builds the preload library there and copies it to $APPF_OUT/lib/TAG.so.
# Run under scripts/heavy.
set -euo pipefail
S="${APPF_OUT:-/tmp/app-faults}"; T="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
W="$(cd "$T/../../.." && pwd)"
tag="$1"; shift
diag=0
if [ "${1:-}" = "--diag" ]; then diag=1; shift; fi
D="$S/var/$tag"
rm -rf "$D/src"
mkdir -p "$D/src" "$S/lib"
cp -r "$W/Cargo.toml" "$W/Cargo.lock" "$W/crates" "$D/src/"
edits=()
for p in "$@"; do edits+=("$(realpath "$p")"); done
cd "$D/src"
if [ "$diag" = 1 ]; then
  patch -p1 -s < "$T/diag.patch"
  cp "$T/diag.rs" crates/asmalloc/src/os/linux/diag.rs
fi
for p in ${edits[@]+"${edits[@]}"}; do python3 -I "$p"; done
CARGO_TARGET_DIR="$S/var/target" cargo build --release -p asmalloc-preload 2>&1 | grep -E "^error|^warning: unused" -A5 || true
cp "$S/var/target/release/libasmalloc_preload.so" "$S/lib/$tag.so"
echo "built $S/lib/$tag.so"
