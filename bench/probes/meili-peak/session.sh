#!/usr/bin/env bash
# One round of Meilisearch for docs/research/meili-peak.md, the allocators in a random order,
# with default-thread's watcher beside it. Run from bench/ of this checkout:
#
#   ../scripts/measure --both-l3 probes/meili-peak/session.sh OUTDIR TAG ALLOCATORS [SUITE]
#
# SUITE is meili-tail (the default: the fill, then two minutes at 70%) or servers-thread
# (the standings' memory job: half a minute at 70%, a silence, a burst, a minute after close;
# only its Meilisearch job is run). Writes OUTDIR/TAG.jsonl (benchrun's results) and
# OUTDIR/TAG.watch.jsonl (the watcher: cgroup memory every 250 ms, the background thread's
# wake-ups, and the counters of a build that exports them). The sampling builds write their
# own files where their entries' environment says (OUTDIR/memstat/, OUTDIR/jestat/).
#
# The entries are local to bench/allocators.toml, not committed:
#
#   [[allocator]]
#   name = "mp-main"                  # build.sh mp-main 169b5c7 asmalloc
#   feature = "asmalloc"
#   build = "mp-main"
#   env = { MP_PAD_XXXXXXXX = "0" }
#   expect = { allocator = "asmalloc", library = "executable" }
#
#   [[allocator]]
#   name = "mp-diag"                  # build.sh mp-diag 169b5c7 asmalloc-memstat peak_edit.py
#   feature = "asmalloc"
#   build = "mp-diag"
#   env = { ASMALLOC_MEMSTAT = "OUTDIR/memstat/meili" }
#   expect = { allocator = "asmalloc", library = "executable" }
#
#   [[allocator]]
#   name = "mp-je"                    # build.sh mp-je 169b5c7 je-stats peak_edit.py
#   feature = "je"
#   build = "mp-je"
#   env = { JEMALLOC_MEMSTAT = "OUTDIR/jestat/meili" }
#   expect = { allocator = "jemalloc", library = "executable" }
#
# and the prototype's entries (the note lists them).
set -euo pipefail
out="$1" tag="$2" allocs="$3" suite="${4:-meili-tail}"
here="$(cd "$(dirname "$0")" && pwd)"
bench="$(cd "$here/../.." && pwd)"
mkdir -p "$out/memstat" "$out/jestat"
only=()
if [ "$suite" = servers-thread ]; then
  only=(--only "=meilisearch hn thread")
fi
exes=()
for f in "$bench"/target/*/apps/meilisearch/release/meilisearch; do
  [ -x "$f" ] && exes+=(--exe "$(readlink -f "$f")")
done
taskset -c 15 python3 -I "$bench/probes/default-thread/watch.py" --out "$out/$tag.watch.jsonl" \
  "${exes[@]}" --loadgen "$bench/target/loadgen/release/loadgen" &
watcher=$!
trap 'kill -TERM $watcher; wait $watcher' EXIT
"$bench/target/release/benchrun" run --suite "$suite" "${only[@]}" \
  --allocators "$allocs" --rounds 1 --mem-cap 12G --out "$out/$tag.jsonl"
