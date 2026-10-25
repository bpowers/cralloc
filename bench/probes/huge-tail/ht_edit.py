#!/usr/bin/env python3
"""The run-time switch for docs/research/huge-tail.md, for measurement only. Applied to a
scratch copy of the repository (build.sh), it puts ht.rs into the allocator as `crate::ht`
and routes the purge walk's view of a region's huge-page part through it, so that one binary
runs main's rule and each candidate (`ASMALLOC_HT`, see ht.rs). It also reports each region's
aged frontier to `Asmalloc::inspect`, and the memstat sampler writes it, with the empty pages
it cannot return above and below it, on each region's map of units.

  ht_edit.py TREE

The allocator's own tests are not built with this edit.
"""
import shutil
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
HERE = Path(__file__).resolve().parent
SRC = TREE / "crates/asmalloc/src"


def sub(path, old, new, count=1, root=SRC):
    p = root / path
    s = p.read_text()
    if s.count(old) != count:
        raise SystemExit(f"{path}: {s.count(old)} matches (want {count}) for {old[:90]!r}")
    p.write_text(s.replace(old, new))


# --- The frontier, and the tail's boundary, in the introspection walk.
sub("stats.rs", """    /// The region's metadata block.
    pub metadata: *const u8,""", """    /// Scratch (huge-tail): the aged frontier, in bytes from the base, at the walk's interval:
    /// the clocked pass leaves units that start below it.
    pub frontier: usize,
    /// Scratch (huge-tail): where the part advised against huge pages again begins, if any.
    pub tail_from: Option<usize>,
    /// The region's metadata block.
    pub metadata: *const u8,""")
sub("back/inspect.rs", """            huge_from: hints.huge_from.map(|from| from as usize * page),
""", """            huge_from: hints.huge_from.map(|from| from as usize * page),
            frontier: super::region::floor(&host, region, self.epoch()) as usize * page,
            tail_from: crate::ht::tail_of(region.index()).map(|from| from as usize * page),
""")

# --- The sampler: the frontier and the stranded empty pages on each region's map.
M = TREE / "bench/alloc/src"
sub("memstat.rs", """    carved_end: usize,
    pages: usize,
    huge_start: usize,
}""", """    carved_end: usize,
    pages: usize,
    huge_start: usize,
    frontier: usize,
    tail_from: usize,
}""", root=M)
sub("memstat.rs", """                pages: occ_pages,
                huge_start: self.huge_start,
            };""", """                pages: occ_pages,
                huge_start: self.huge_start,
                frontier: base + region.frontier,
                tail_from: region.tail_from.map_or(0, |t| base + t),
            };""", root=M)
# Per unit: the resident pages wholly free, in a unit that is not wholly free (what the purge
# cannot return there), split by whether the unit starts at or above the frontier rounded up to
# a unit (what the candidate rule could reach).
sub("memstat.rs", """        let mut unit: Option<(usize, u64, u64, bool)> = None;""",
    """        let mut unit: Option<(usize, u64, u64, bool)> = None;
        let mut empties = 0u64;
        let (mut stuck_above, mut stuck_below, mut units_above) = (0u64, 0u64, 0u64);
        let reach = o.frontier.next_multiple_of(HUGE);""", root=M)
sub("memstat.rs", """        let mut close = |c: u64, u: u64, res: bool| {
            let ch = if !res || c == 0 { '.' } else { UNIT_CHARS[bucket(u, c)] };
            if res && c > 0 {
                units[bucket(u, c)] += 1;
            }
            let _ = maps.write_char(ch);
        };""", """        let mut close = |s: usize, c: u64, u: u64, res: bool, e: u64| {
            let ch = if !res || c == 0 { '.' } else { UNIT_CHARS[bucket(u, c)] };
            if res && c > 0 {
                units[bucket(u, c)] += 1;
            }
            if res && u > 0 {
                if s >= reach {
                    stuck_above += e;
                } else {
                    stuck_below += e;
                }
            }
            if res && c > 0 && s >= reach {
                units_above += 1;
            }
            let _ = maps.write_char(ch);
        };""", root=M)
sub("memstat.rs", """                if let Some((s, c, u, res)) = unit
                    && s != start
                {
                    close(c, u, res);
                    unit = None;
                }
                let (s, c, u, res) = unit.unwrap_or((start, 0, 0, false));
                unit = Some((s, c + carved, u + used, res || resident));""", """                if let Some((s, c, u, res)) = unit
                    && s != start
                {
                    close(s, c, u, res, empties);
                    unit = None;
                    empties = 0;
                }
                if resident && carved == PAGE as u64 && used == 0 {
                    empties += 1;
                }
                let (s, c, u, res) = unit.unwrap_or((start, 0, 0, false));
                unit = Some((s, c + carved, u + used, res || resident));""", root=M)
sub("memstat.rs", """        if let Some((_, c, u, res)) = unit {
            close(c, u, res);
        }
        if o.huge_start < o.carved_end {
            let _ = writeln!(maps);
        }""", """        if let Some((s, c, u, res)) = unit {
            close(s, c, u, res, empties);
        }
        if o.huge_start < o.carved_end {
            let _ = writeln!(
                maps,
                " frontier_mib={} tail_mib={} units_above={} stuck_above_kib={} stuck_below_kib={}",
                o.frontier.saturating_sub(o.base) >> 20,
                o.tail_from.saturating_sub(o.base) >> 20,
                units_above,
                stuck_above * 4,
                stuck_below * 4
            );
        }""", root=M)

# --- The switch.
shutil.copy(HERE / "ht.rs", SRC / "ht.rs")
sub("lib.rs", "mod heap;\n", "mod heap;\npub mod ht;\n")
if (HERE / "ht_proto.py").exists():
    exec((HERE / "ht_proto.py").read_text(), {"sub": sub, "SRC": SRC, "TREE": TREE, "HERE": HERE})
