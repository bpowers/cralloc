#!/usr/bin/env bash
# The C loops of cloop.c with each allocator preloaded, interleaved over RF_ROUNDS rounds
# (default 3), each cell the fastest round. Pinned to RF_CPU (default 13).
#
#   compare.sh [FORM...]          default: pair:32 burst:8:32 burst:32:32 mixed:1:512
#
# Allocators: glibc (nothing preloaded), the stand-ins, jemalloc, mimalloc and tcmalloc from
# BASELINES (default: the main checkout's bench/baselines/out, read-only). tcmalloc runs twice:
# as built (per-thread caches, because glibc owns the rseq area) and with glibc's rseq turned
# off so that its per-CPU caches engage; the stand-ins need glibc's rseq and cannot run there.
# cralloc-cxx is the same with the C++ operators exported, CRALLOC_CXX_LIB (a release build of
# crates/cralloc-preload/tests/programs/cxx_exports as of commit d12eacf, the last with them). cralloc is the real library, CRALLOC_LIB (default: this checkout's release build of
# libcralloc_preload.so). Builds: cloop-plt (PLT), cloop-ibt (IBT PLT, .plt.sec), cloop-noplt
# (call through the GOT), and cloop-static, which links cralloc in and calls its routines
# directly; it runs only as cralloc, with nothing preloaded.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$here/out
memlimit=$here/../../../scripts/memlimit
common=$(git -C "$here" rev-parse --path-format=absolute --git-common-dir)
base=${BASELINES:-$(dirname "$common")/bench/baselines/out}
forms=("$@")
[ ${#forms[@]} -gt 0 ] || forms=(pair:32 burst:8:32 burst:32:32 mixed:1:512)
rounds=${RF_ROUNDS:-3}
builds=(${RF_BUILDS:-plt ibt noplt static})
# The loops: cloop (C), or cxxloop (C++ new and delete; plt and noplt builds only).
prog=${RF_PROG:-cloop}
repo=$(git -C "$here" rev-parse --show-toplevel)

declare -A lib=(
  [glibc]=""
  [cralloc]="${CRALLOC_LIB:-$repo/target/release/libcralloc_preload.so}"
  [cralloc-cxx]="${CRALLOC_CXX_LIB:-}"
  [asm-best]="$out/libstandin-best.so"
  [asm-rust]="$out/libstandin-rust.so"
  [asm-two]="$out/libstandin-two.so"
  [asm-best-c0]="$out/libstandin-best-c0.so"
  [asm-best-c1]="$out/libstandin-best-c1.so"
  [asm-best-c2]="$out/libstandin-best-c2.so"
  [jemalloc]="$base/jemalloc/libjemalloc.so"
  [mimalloc]="$base/mimalloc/libmimalloc.so"
  [tcmalloc]="$base/tcmalloc/libtcmalloc.so"
  [tcmalloc-percpu]="$base/tcmalloc/libtcmalloc.so"
)
order=(${RF_ALLOCS:-glibc cralloc asm-best asm-rust jemalloc mimalloc tcmalloc tcmalloc-percpu})

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for r in $(seq "$rounds"); do
  for b in "${builds[@]}"; do
    for a in "${order[@]}"; do
      if [ "$b" = static ]; then
        [ "$a" = cralloc ] || continue
      fi
      envs=()
      [ -n "${lib[$a]}" ] && [ "$b" != static ] && envs+=("LD_PRELOAD=${lib[$a]}")
      [ "$a" = tcmalloc-percpu ] && envs+=("GLIBC_TUNABLES=glibc.pthread.rseq=0")
      MEMLIMIT_QUIET=1 "$memlimit" -m 4G -t 10m -- env "${envs[@]}" "$out/$prog-$b" "${forms[@]}" \
        | sed "s/^/$b $a /" >> "$tmp/r$r"
    done
  done
done
# Best of the rounds per (build, allocator, form).
cat "$tmp"/r* | awk '
  { k = $1 " " $2 " " $3; if (!(k in best) || $4 < best[k]) { best[k] = $4; ins[k] = $5 }
    if (!(k in worst) || $4 > worst[k]) worst[k] = $4; seen[k] = 1 }
  END { for (k in seen) printf "%s %.2f +%.2f %.1f\n", k, best[k], worst[k] - best[k], ins[k] }' \
  | sort -k3,3 -k1,1 -k4,4n | column -t
