#!/usr/bin/env bash
# The whole bill for this machine, on any aarch64 Linux: finds its CPUs from sysfs (`topo`), runs
# each probe through the repository's measurement scripts when they are there, and writes one
# file for each into OUT.
#
#   bench/roofline/machine/run.sh OUT [probe...]
#
# Probes: lines moves hier streams kernel huge clock, all of them by default. About ten minutes,
# and the machine should be otherwise quiet. Build first:
#   cargo build --release --manifest-path bench/roofline/machine/Cargo.toml
#   cargo build --release --manifest-path bench/roofline/oscost/Cargo.toml --bin faults
set -u

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
out=${1:?usage: run.sh OUT [probe...]}
shift
mkdir -p "$out"
bin=$here/target/release
faults=$here/../oscost/target/release/faults
wanted=("$@")
[ ${#wanted[@]} -gt 0 ] || wanted=(lines moves hier streams kernel huge clock)

"$bin/topo" > "$out/topo.txt" || exit 1
# shellcheck disable=SC1091
. "$out/topo.txt"
near=${BIG_NEAR:-$BIG}
far=${BIG_FAR:-$near}
others=$OTHERS

# Everything through the measurement lock and the memory cap, where this checkout has them.
run() {
  local name=$1 cap=$2
  shift 2
  echo "== $name"
  if [ -x "$repo/scripts/measure" ]; then
    "$repo/scripts/measure" --both-l3 "$repo/scripts/memlimit" -m "$cap" -t 20m -- "$@" > "$out/$name.txt" 2>&1
  else
    "$@" > "$out/$name.txt" 2>&1
  fi
}

want() {
  local w
  for w in "${wanted[@]}"; do [ "$w" = "$1" ] && return 0; done
  return 1
}

if want lines; then
  run regs 1G "$bin/lines" regs
  run granule-near 2G "$bin/lines" granule "$BIG" "$near"
  run granule-far 2G "$bin/lines" granule "$BIG" "$far"
  [ -n "$LITTLE" ] && run granule-little 2G "$bin/lines" granule "$BIG" "$LITTLE"
  run neighbour 6G "$bin/lines" neighbour "$BIG" 2048
fi

if want moves; then
  for pair in "near $near" "far $far" ${LITTLE:+"little $LITTLE"}; do
    set -- $pair
    run "pingpong-$1" 1G "$bin/moves" pingpong "$BIG" "$2"
    run "atomics-$1" 1G "$bin/moves" atomics "$BIG" "$2"
    run "nextuser-$1" 1G "$bin/moves" nextuser "$BIG" "$2"
    run "nextread-$1" 1G "$bin/moves" nextread "$BIG" "$2"
  done
fi

if want hier; then
  for cpu in "$BIG" ${LITTLE:+"$LITTLE"}; do
    run "latency-$cpu-small" 10G "$bin/hier" latency "$cpu" small 4096
    run "latency-$cpu-huge" 10G "$bin/hier" latency "$cpu" huge 4096
    run "tlb-$cpu" 8G "$bin/hier" tlb "$cpu"
    run "tlbsets-$cpu" 2G "$bin/hier" tlbsets "$cpu"
    run "sets-$cpu" 4G "$bin/hier" sets "$cpu"
    run "unit-$cpu" 2G "$bin/hier" unit "$cpu"
    run "bandwidth-$cpu" 8G "$bin/hier" bandwidth "$cpu"
  done
  run bandwidth-domain 15G "$bin/hier" bandwidth "$BIG,$others"
  run prefetch-small 6G "$bin/hier" prefetch "$BIG" small 2048
  run prefetch-huge 6G "$bin/hier" prefetch "$BIG" huge 2048
  run runlength 6G "$bin/hier" run "$BIG" 2048
fi

if want streams; then
  run stagger-small-64 12G "$bin/streams" stagger "$BIG" small 64 1024
  run stagger-huge-64 12G "$bin/streams" stagger "$BIG" huge 64 1024
  run rule-small 12G "$bin/streams" rule "$BIG" small
  run origin 4G "$bin/streams" origin "$BIG"
  run sawtooth 6G "$bin/streams" sawtooth "$BIG" 512
  run pointers 6G "$bin/streams" pointers "$BIG" 2048
  run magazine 4G "$bin/streams" magazine "$BIG" 512
  [ -n "$LITTLE" ] && run magazine-little 4G "$bin/streams" magazine "$LITTLE" 512
fi

if want kernel; then
  run purge-alone 2G "$bin/kcost" purge "$BIG"
  run purge-others 2G "$bin/kcost" purge "$BIG,$others"
  run purge-all 2G "$bin/kcost" purge "$BIG,$(echo "$ALL" | tr ',' '\n' | grep -vx "$BIG" | paste -sd,)"
  run calls-big 2G "$bin/kcost" calls "$BIG"
  [ -n "$LITTLE" ] && run calls-little 2G "$bin/kcost" calls "$LITTLE"
  run barrier 2G "$bin/kcost" barrier "$BIG" "$near,$far${LITTLE:+,$LITTLE}"
  for mib in 8 64 256 1024; do
    RF_CPUS=$BIG run "faults-$mib" 8G "$faults" single "$mib" 5
  done
  RF_CPUS=$BIG,$others run faults-threads 8G "$faults" threads 64 5
fi

if want huge; then
  RF_CPUS=$BIG run faults-split 15G "$faults" split
  RF_CPUS=$BIG run faults-advice 15G "$faults" advice 1000
  run cold 15G "$bin/worth" cold "$BIG"
  run contig 15G "$bin/worth" contig "$BIG"
  run churn 15G "$bin/worth" churn "$BIG" 3
  run build 15G "$bin/worth" build "$BIG"
  RF_CPUS=$BIG run faults-khuge 15G "$faults" khuge 120
fi

if want clock; then
  run ramp 1G "$bin/clockidle" ramp "$BIG" 2 40
  [ -n "$LITTLE" ] && run ramp-little 1G "$bin/clockidle" ramp "$LITTLE" 2 40
  run load-one 1G "$bin/clockidle" load "$BIG"
  run load-domain 1G "$bin/clockidle" load "$DOMAIN"
  run load-all 1G "$bin/clockidle" load "$ALL"
  run wake 2G "$bin/clockidle" wake "$BIG" "$near"
fi
echo "results in $out"
