#!/usr/bin/env bash
# Builds every candidate listed in a file of build.sh argument lines (default builds-crates.txt),
# one after another. Run under scripts/heavy. A NAME given after the file builds only those.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
list="${1:-$here/builds-crates.txt}"
shift || true
while read -r name crate version rest; do
  case "$name" in ''|'#'*) continue ;; esac
  if [ $# -gt 0 ] && ! printf '%s\n' "$@" | grep -qx "$name"; then continue; fi
  # shellcheck disable=SC2086
  "$here/build.sh" "$name" "$crate" "$version" $rest
done <"$list"
