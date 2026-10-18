#!/usr/bin/env bash
# Links incumbent.c against mimalloc's static object once per placement, with mi_free and
# mi_malloc at P bytes past a 64-byte boundary, into OUT/incumbent-P. Their sections are copied
# with alignment 1 and ordered after a P-byte pad (GNU ld's --section-ordering-file); nothing
# else about them changes.
#
#   build-incumbent.sh OUT [P ...]        (default P: 0 to 63)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
common=$(cd "$root" && cd "$(git rev-parse --git-common-dir)/.." && pwd)
mi="${BENCH_BASELINES:-$common/bench/baselines/out}/mimalloc/static.o"
out=$1
shift
places=("$@")
[ ${#places[@]} -gt 0 ] || places=($(seq 0 63))
mkdir -p "$out"
objcopy --set-section-alignment .text.mi_free=1 --set-section-alignment .text.mi_malloc=1 "$mi" "$out/mi.o"
cc -O2 -DALLOC_MALLOC=mi_malloc -DALLOC_FREE=mi_free -c "$here/incumbent.c" -o "$out/incumbent.o"
for p in "${places[@]}"; do
  printf '.section .text.ck_pad_free,"ax"\n.balign 64\n.skip %d, 0xcc\n.section .text.ck_pad_malloc,"ax"\n.balign 64\n.skip %d, 0xcc\n' "$p" "$p" \
    | as -o "$out/pad-$p.o"
  cat > "$out/order-$p.ld" <<EOF
.text : {
  *(.text.ck_pad_free)
  *(.text.mi_free)
  *(.text.ck_pad_malloc)
  *(.text.mi_malloc)
}
EOF
  cc -O2 -Wl,--section-ordering-file="$out/order-$p.ld" "$out/incumbent.o" "$out/pad-$p.o" "$out/mi.o" -lpthread \
    -o "$out/incumbent-$p"
done
