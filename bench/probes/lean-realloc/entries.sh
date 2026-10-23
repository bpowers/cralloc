#!/usr/bin/env bash
# usage: entries.sh NAME...
# Prints the allocators.toml entries for the builds `build.sh` made, `NAME@padNN` with
# `build = "lr-NAME-pNN"`, to append to bench/allocators.toml for a session and take out
# after it (they are local, never committed).
set -euo pipefail
for name in "$@"; do
  for pad in 00 16 32 48 64; do
    cat <<EOF

[[allocator]]
name = "$name@pad$pad"
feature = "asmalloc"
build = "lr-$name-p$pad"
expect = { allocator = "asmalloc", library = "executable" }
EOF
  done
done
