#!/usr/bin/env bash
# usage: analyze.sh NAME [TOP] ; NAME like AT-t4 in bench/target/perf (NAME.c2c, NAME.trace)
set -u
here=$(cd "$(dirname "$0")" && pwd)
cd "$here/../../target/perf"
n=$1
perf script -i "$n.c2c" -F cpu,addr,weight,data_src,ip,sym 2>/dev/null > "$n.txt"
python3 "$here/attrib.py" "$n.txt" "$n.trace" "${2:-20}"
