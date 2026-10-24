#!/usr/bin/env bash
# Where dTLB walks land, by asmalloc region: IBS op samples of the HashMap job and the
# screening's programs under the switch (main, and the 2 MiB arm), on the heavy cores (the
# shares are what matter, not the time). From the checkout:
#   scripts/heavy bench/probes/whole-block-huge/sessions/attrib.sh OUTDIR
set -euo pipefail
out="$1"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
root="$(pwd)"
here="$(pwd)/bench/probes/whole-block-huge"
B="$(pwd)/bench/target/wbh"
R="${RUST_SCREEN_ROOT:-/home/bpowers/src/asmalloc/.claude/worktrees/agent-ab8583755fe797b69/target/rust-screen}"
W="$(pwd)/target/wbh-work/attrib"
mkdir -p "$W"
for s in m0f002d0 m1f002d0; do
  rm -f "$out/map-$s.txt"
  ASMALLOC_WBH=$s WBH_MAP="$out/map-$s.txt" BENCH_CPUS=12 scripts/memlimit -m 3G -t 5m -- \
    python3 -I "$here/ibs.py" record "$out/map-$s.data" 5003 -- taskset -c 12 "$B/release/workload" map kind=hash n=1M reps=5 > /dev/null
  python3 -I "$here/ibs.py" classify "$out/map-$s.data" "$out/map-$s.txt" | tee "$out/map-$s.class"
  rm -f "$out/map-$s.data"
  rm -f "$out/tpch-$s.txt"
  (cd "$W" && rm -rf out && ASMALLOC_WBH=$s WBH_MAP="$out/tpch-$s.txt" "$root/scripts/memlimit" -m 8G -t 5m -- \
    python3 -I "$here/ibs.py" record "$out/tpch-$s.data" 50021 -- taskset -c 12-15 \
      env LD_PRELOAD="$B/libasmalloc_preload.so" "$R/bin/tpchgen/tpchgen-cli" parquet -s 1 --output-dir=out > /dev/null)
  python3 -I "$here/ibs.py" classify "$out/tpch-$s.data" "$out/tpch-$s.txt" | tee "$out/tpch-$s.class"
  rm -f "$out/tpch-$s.data"
done
for prog in jaq rust-analyzer; do
  s=m0f002d0
  rm -f "$out/$prog-$s.txt"
  case "$prog" in
    jaq) cmd=("$R/bin/jaq/jaq" -c '.features | map(select(.properties.BLOCK_NUM != null)) | group_by(.properties.STREET) | map({s: .[0].properties.STREET, n: length, lots: map(.properties.LOT_NUM)}) | length' "$R/inputs/dl/citylots.json"); cpus=12 ;;
    rust-analyzer) cmd=(env RUST_SRC_PATH="$R/inputs/rust-src-1.99.1/rust-src/lib/rustlib/src/rust/library" CARGO_NET_OFFLINE=true CARGO="$R/toolchain-cargo" "$R/bin/rust-analyzer/rust-analyzer" analysis-stats --disable-build-scripts --disable-proc-macros "$R/inputs/ra/ripgrep-15.2.0"); cpus=12-15 ;;
  esac
  ASMALLOC_WBH=$s WBH_MAP="$out/$prog-$s.txt" scripts/memlimit -m 8G -t 5m -- \
    python3 -I "$here/ibs.py" record "$out/$prog-$s.data" 50021 -- taskset -c $cpus \
      env LD_PRELOAD="$B/libasmalloc_preload.so" "${cmd[@]}" > /dev/null 2>&1
  python3 -I "$here/ibs.py" classify "$out/$prog-$s.data" "$out/$prog-$s.txt" | tee "$out/$prog-$s.class"
  rm -f "$out/$prog-$s.data"
done
