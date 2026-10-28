#!/usr/bin/env python3
"""Turns a scratch copy of the tree into the trough-gap probe's instrumented builds
(docs/research/trough-gap.md). Probe only, never product code:

    python3 -I tg_edit.py TREE

- The allocator's introspection walk (`Asmalloc::inspect`) also reports each region's aged
  frontier (the page below which the clocked pass leaves units alone), its tail boundary and
  raw tail word, and the clean bits of its carved pages (`Visitor::clean`).
- bench-alloc's memstat sampler is replaced by this probe's (memstat.rs here), which attributes
  every resident page of every region by what the rules would do with it, and does not count
  requests.
- bench-alloc's jemalloc module is replaced by this probe's (je.rs here), and a feature
  `je-stats` builds tikv-jemallocator with jemalloc's statistics on, so that the drift
  workload's `account_every` writes jemalloc's account (`JEMALLOC_ACCT`) at the same moments
  as asmalloc's.
"""
import shutil
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
HERE = Path(__file__).resolve().parent


def sub(rel, old, new, count=1):
    p = TREE / rel
    s = p.read_text()
    if s.count(old) != count:
        raise SystemExit(f"{rel}: {s.count(old)} matches for {old[:80]!r}")
    p.write_text(s.replace(old, new))


# The walk's report of a region.
sub(
    "crates/asmalloc/src/stats.rs",
    """    /// Bytes of the metadata block the class's layout uses.
    pub metadata_len: usize,
}""",
    """    /// Bytes of the metadata block the class's layout uses.
    pub metadata_len: usize,
    /// Scratch (trough-gap): the aged frontier, in bytes from the base.
    pub frontier: usize,
    /// Scratch: the tail boundary, in bytes from the base; zero for none.
    pub tail_from: usize,
    /// Scratch: the raw tail word.
    pub tail_word: u64,
}""",
)
sub(
    "crates/asmalloc/src/stats.rs",
    """    /// A run of consecutive free slots in the bitmap of the region last reported""",
    """    /// Scratch (trough-gap): clean-bitmap word `index` of the region last reported: a set bit
    /// is a page the back end believes returned.
    fn clean(&mut self, index: usize, bits: u64) {
        let _ = (index, bits);
    }

    /// A run of consecutive free slots in the bitmap of the region last reported""",
)
sub(
    "crates/asmalloc/src/back/inspect.rs",
    """        let base = self.space.region(region).as_ptr().cast_const();
        visitor.region(&Region {""",
    """        let base = self.space.region(region).as_ptr().cast_const();
        let tail_word = host.tail(region).load();
        let tail_from = super::tail::Tail::unpack(tail_word).from as usize
            * layout.huge_pages() as usize
            * page;
        let frontier = super::region::floor(&host, region, self.epoch()) as usize * page;
        visitor.region(&Region {""",
)
sub(
    "crates/asmalloc/src/back/inspect.rs",
    """            metadata_len: layout.block_bytes(),
        });""",
    """            metadata_len: layout.block_bytes(),
            frontier,
            tail_from,
            tail_word,
        });
        for w in 0..layout.carved_pages(carved).div_ceil(BITS) {
            visitor.clean(w as usize, host.meta(region, layout.clean_word(w)).load());
        }""",
)

# bench-alloc: the probe's sampler and jemalloc module, and the statistics feature.
shutil.copy(HERE / "memstat.rs", TREE / "bench/alloc/src/memstat.rs")
shutil.copy(HERE / "je.rs", TREE / "bench/alloc/src/je.rs")
sub(
    "bench/alloc/Cargo.toml",
    """je = ["dep:tikv-jemallocator"]""",
    """je = ["dep:tikv-jemallocator"]
# Scratch (trough-gap): jemalloc with its statistics, for its account at a snapshot.
je-stats = ["je", "tikv-jemallocator/stats"]""",
)
sub(
    "bench/alloc/build.rs",
    """["asmalloc-memstat", "asmalloc-background", "asmalloc-domains", "live-bytes", "memacct"]""",
    """["asmalloc-memstat", "asmalloc-background", "asmalloc-domains", "live-bytes", "memacct", "je-stats"]""",
)
sub(
    "bench/alloc/src/lib.rs",
    """    #[cfg(feature = "asmalloc-memstat")]
    return memstat::active();
    #[cfg(not(feature = "asmalloc-memstat"))]
    false""",
    """    #[cfg(feature = "asmalloc-memstat")]
    return memstat::active();
    #[cfg(feature = "je")]
    return je::snapshot_supported();
    #[cfg(not(any(feature = "asmalloc-memstat", feature = "je")))]
    false""",
)
sub(
    "bench/alloc/src/lib.rs",
    """    #[cfg(feature = "asmalloc-memstat")]
    memstat::snapshot(tag);
    #[cfg(not(feature = "asmalloc-memstat"))]
    let _ = tag;""",
    """    #[cfg(feature = "asmalloc-memstat")]
    memstat::snapshot(tag);
    #[cfg(feature = "je")]
    je::snapshot(tag);
    #[cfg(not(any(feature = "asmalloc-memstat", feature = "je")))]
    let _ = tag;""",
)
print("tg_edit: applied")
