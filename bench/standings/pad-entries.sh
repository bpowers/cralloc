#!/usr/bin/env bash
# usage: pad-entries.sh [ALLOC...]   (ALLOC: cra jem tcm mim gli, default all five, or cro)
# Prints the allocators.toml entries for pad-build.sh's builds, `NAME@padNN` with
# `build = "ALLOC-pNN"` and the environment and checks of the entry NAME, to append to
# bench/allocators.toml for the sessions and take out after them (local, never committed).
set -euo pipefail
allocs=("$@")
[ ${#allocs[@]} -gt 0 ] || allocs=(cra jem tcm mim gli)
case "$(uname -m)" in
  aarch64) default_pads="000 032 064 096 128" ;;
  *) default_pads="00 16 32 48 64" ;;
esac
for a in "${allocs[@]}"; do
  case "$a" in
    cra) name=cralloc feature=cralloc extra='expect = { allocator = "cralloc", library = "executable" }' ;;
    cro) name=cralloc-outline feature=cralloc-outline extra='expect = { allocator = "cralloc", library = "executable" }' ;;
    jem) name=jemalloc feature=je extra='expect = { allocator = "jemalloc", library = "executable" }' ;;
    tcm) name=tcmalloc-percpu feature=tc
         extra='env = { GLIBC_TUNABLES = "glibc.pthread.rseq=0" }
expect = { allocator = "tcmalloc", mode = "per-cpu", library = "executable" }' ;;
    mim) name=mimalloc feature=mi extra='expect = { allocator = "mimalloc", library = "executable", version = "30503" }' ;;
    gli) name=glibc feature="" extra='expect = { allocator = "glibc", library = "libc.so.6" }' ;;
    *) echo "pad-entries: unknown allocator $a" >&2; exit 2 ;;
  esac
  for pad in ${PADS:-$default_pads}; do
    cat <<EOF

[[allocator]]
name = "$name@pad$pad"
feature = "$feature"
build = "$a-p$pad"
$extra
EOF
  done
done
