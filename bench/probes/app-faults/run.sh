#!/usr/bin/env bash
# run.sh NAME LIB [prefix command...]
# Runs candidate NAME from the screening tree with LIB preloaded (LIB "-" for glibc) in a
# private work dir, optionally wrapped by a prefix command (strace, perf ...). Prints nothing
# of the program's output.
set -euo pipefail
name="$1"; lib="$2"; shift 2
R="$RUST_SCREEN_ROOT"
S="${APPF_OUT:-/tmp/app-faults}"; T="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
W="$S/work/$name"
mkdir -p "$W"
cd "$W"
pre=()
if [ "$lib" != "-" ]; then pre=(env "LD_PRELOAD=$lib"); else pre=(env); fi
case "$name" in
  tpchgen)
    rm -rf out
    cmd=("$R/bin/tpchgen/tpchgen-cli" parquet -s 1 --output-dir=out) ;;
  gix-hours)
    cmd=("$R/bin/gix/ein" tool estimate-hours --file-stats --line-stats "$R/inputs/repo/cargo.git" c24e1064277fe51ab72011e2612e556ac56addf7) ;;
  ruff-check)
    cmd=("$R/bin/ruff/ruff" check --no-cache --select ALL --exit-zero -q --output-format concise "$R/inputs/Python-3.13.0/Lib") ;;
  datafusion)
    cmd=("$R/bin/datafusion/datafusion-cli" -q -f "$R/inputs/tpch/run.sql") ;;
  pulldown-cmark)
    cmd=("$R/bin/pulldown-cmark/pulldown-cmark" -T -F -S "$R/inputs/md/corpus.md" "$R/inputs/md/corpus.md" "$R/inputs/md/corpus.md" "$R/inputs/md/corpus.md") ;;
  *) echo "unknown $name" >&2; exit 2 ;;
esac
exec "$@" "${pre[@]}" "${cmd[@]}" >/dev/null 2>"$W/stderr"
