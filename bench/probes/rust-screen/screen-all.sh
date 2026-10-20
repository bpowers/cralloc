#!/usr/bin/env bash
# Screens candidates end to end: three interleaved timed rounds and a perf profile (each under
# scripts/measure, with --both-l3 when the candidate's CPUs reach the second L3), then the
# allocation profile (under scripts/heavy). Everything runs under an 8 GiB cap unless
# SCREEN_MEM says otherwise.
#
# Usage: screen-all.sh NAME...      SCREEN_STEPS=time,perf,profile picks steps.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
mem="${SCREEN_MEM:-8G}"
steps="${SCREEN_STEPS:-time,perf,profile}"
for name in "$@"; do
  cpus="$(python3 "$here/screen.py" list | awk -v n="$name" '$1 == n { sub("cpus=", "", $2); print $2 }')"
  both=()
  # CPUs 8-15 and 24-31 are the second L3.
  if python3 -c "import sys; c=set();
for p in sys.argv[1].split(','):
    a,_,b=p.partition('-'); c.update(range(int(a), int(b or a)+1))
sys.exit(0 if c & set(range(8,16)) | (c & set(range(24,32))) else 1)" "$cpus"; then
    both=(--both-l3)
  fi
  case ",$steps," in *,time,*)
    "$repo/scripts/measure" "${both[@]}" "$repo/scripts/memlimit" -m "$mem" -t 60m -- python3 "$here/screen.py" time "$name" ;;
  esac
  case ",$steps," in *,perf,*)
    "$repo/scripts/measure" "${both[@]}" "$repo/scripts/memlimit" -m "$mem" -t 30m -- python3 "$here/screen.py" perf "$name" ;;
  esac
  case ",$steps," in *,profile,*)
    "$repo/scripts/heavy" "$repo/scripts/memlimit" -m "$mem" -t 60m -- python3 "$here/screen.py" profile "$name" >/dev/null ;;
  esac
  python3 "$here/screen.py" report "$name"
done
