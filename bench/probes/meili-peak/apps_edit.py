#!/usr/bin/env python3
"""After peak_edit.py: gives every other app's driver the account builds' two features
(`asmalloc-memstat`, `je-stats`), as peak_edit.py gives Meilisearch's, so that build.sh can
make account builds of them (PROGRAM=qdrant, ...). Never product code: apply it to a copy.

  apps_edit.py TREE
"""
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
OLD = 'je = ["bench-alloc/je"]\n'
for manifest in sorted((TREE / "bench/apps").glob("*/Cargo.toml")):
    text = manifest.read_text()
    if OLD in text and "asmalloc-memstat" not in text:
        manifest.write_text(text.replace(
            OLD, OLD + 'je-stats = ["bench-alloc/je-stats"]\nasmalloc-memstat = ["bench-alloc/asmalloc-memstat"]\n', 1))
        print("account features added to", manifest.relative_to(TREE))
