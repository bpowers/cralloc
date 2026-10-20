#!/usr/bin/env bash
# Home Assistant core at release 2026.10.2, as a shallow git checkout: ruff honours
# .gitignore files only inside a git repository, as in the project's CI. The commit id is
# the checksum.
set -euo pipefail
dest="$1"
tag=2026.10.2
commit=8d7fd8b68677c2585be36ec86043308f403875b8

git -c advice.detachedHead=false clone -q --depth 1 --branch "$tag" \
  https://github.com/home-assistant/core "$dest/ha-core"
got=$(git -C "$dest/ha-core" rev-parse HEAD)
if [ "$got" != "$commit" ]; then
  echo "ruff/fetch.sh: tag $tag is at $got, expected $commit" >&2
  exit 1
fi
# A clean tree: nothing changed by checkout filters or attributes.
if [ -n "$(git -C "$dest/ha-core" status --porcelain)" ]; then
  echo "ruff/fetch.sh: the checkout differs from commit $commit" >&2
  exit 1
fi
