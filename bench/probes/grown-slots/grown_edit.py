#!/usr/bin/env python3
"""The run-time switch for docs/research/grown-slots.md, for measurement only. Applied to a
scratch copy of the repository (build.sh), it puts gs.rs into the allocator as `crate::gs` and
routes the budget for kept grown blocks (`Back::keep_grown`) through it, so that one binary runs
main's rule and each candidate. It also counts, when asked, what returned each large slot's
pages and how much of that later claims and growth in place took back.

  grown_edit.py TREE

See gs.rs for `ASMALLOC_GROWN`. The size word of a large slot gets three fields (`kept`, the
`tag` of what last returned its pages, and `when`, the fine count then) in bits the size never
uses; the allocator's own tests are not built with this edit.
"""
import shutil
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
HERE = Path(__file__).resolve().parent
SRC = TREE / "crates/asmalloc/src"


def sub(path, old, new, count=1):
    p = SRC / path
    s = p.read_text()
    if s.count(old) != count:
        raise SystemExit(f"{path}: {s.count(old)} matches (want {count}) for {old[:90]!r}")
    p.write_text(s.replace(old, new))


shutil.copy(HERE / "gs.rs", SRC / "gs.rs")
sub("lib.rs", "mod heap;\n", "mod heap;\npub mod gs;\n")

# --- The size word's new fields.
L = "back/layout.rs"
sub(L, """    pub(crate) unwritten: bool,
}

impl Recorded {
    const HUGE: u64 = 1 << 63;
    const UNWRITTEN: u64 = 1 << 62;

    #[inline(always)]
    pub(crate) const fn unpack(word: u64) -> Recorded {
        Recorded {
            size: (word & !(Recorded::HUGE | Recorded::UNWRITTEN)) as usize,
            huge: word & Recorded::HUGE != 0,
            unwritten: word & Recorded::UNWRITTEN != 0,
        }
    }
""", """    pub(crate) unwritten: bool,
    /// Scratch (grown-slots): a kept grown slot, counted in `crate::gs`'s tally.
    pub(crate) kept: bool,
    /// Scratch (grown-slots): what last returned the slot's pages, and the fine count then.
    pub(crate) tag: u8,
    pub(crate) when: u16,
}

impl Recorded {
    const HUGE: u64 = 1 << 63;
    const UNWRITTEN: u64 = 1 << 62;
    const KEPT: u64 = 1 << 61;
    const SIZE: u64 = (1 << 40) - 1;

    #[inline(always)]
    pub(crate) const fn unpack(word: u64) -> Recorded {
        Recorded {
            size: (word & Recorded::SIZE) as usize,
            huge: word & Recorded::HUGE != 0,
            unwritten: word & Recorded::UNWRITTEN != 0,
            kept: word & Recorded::KEPT != 0,
            tag: ((word >> 56) & 0xf) as u8,
            when: ((word >> 40) & 0xffff) as u16,
        }
    }
""")
sub(L, """        self.size as u64 | huge | unwritten
    }""", """        let kept = if self.kept { Recorded::KEPT } else { 0 };
        self.size as u64
            | huge
            | unwritten
            | kept
            | ((self.tag as u64 & 0xf) << 56)
            | ((self.when as u64) << 40)
    }""")

# --- Region: the clean pages a claim clears under its block; growth in place; the walk's tags.
R = "back/region.rs"
sub(R, "    pub(crate) revived: usize,\n}", "    pub(crate) revived: usize,\n    /// Scratch (grown-slots): clean pages under the block a large claim cleared.\n    pub(crate) under: usize,\n}")
sub(R, "        revived: (cleaned * page).saturating_sub(claimed),\n",
    "        revived: (cleaned * page).saturating_sub(claimed),\n        under: 0,\n")
sub(R, """            resident: layout.size(),
            revived: 0,
        };""", """            resident: layout.size(),
            revived: 0,
            under: 0,
        };""")
sub(R, """    let mut clean = 0usize;
    for (w, mask) in page_words(unit.start, unit.end) {
        let word = host.meta(region, layout.clean_word(w));
        let seen = word.load() & mask;
        clean += count(seen);
        let inside = mask & below(w, end);
""", """    let mut clean = 0usize;
    let mut under = 0usize;
    for (w, mask) in page_words(unit.start, unit.end) {
        let word = host.meta(region, layout.clean_word(w));
        let seen = word.load() & mask;
        clean += count(seen);
        let inside = mask & below(w, end);
        under += (seen & inside).count_ones() as usize;
""")
sub(R, """        resident: layout.size() - clean * layout.page_size(),
        revived: 0,
    }""", """        resident: layout.size() - clean * layout.page_size(),
        revived: 0,
        under,
    }""")
sub(R, """        let word = host.meta(region, layout.clean_word(w));
        if word.load() & inside != 0 {
            word.fetch_and(!inside);
        }""", """        let word = host.meta(region, layout.clean_word(w));
        if word.load() & inside != 0 {
            let old = word.fetch_and(!inside);
            crate::gs::cover((old & inside).count_ones() as usize * layout.page_size(), now.tag, now.when);
        }""")
# The walk's purge of a unit: before the pages go, the charge of any kept slot in it; after,
# every slot's tag, and the kept tally.
sub(R, """            release_range(host, region, first, last, low, w);
            return (Purge::Busy, read);
        }
    }
""", """            release_range(host, region, first, last, low, w);
            return (Purge::Busy, read);
        }
    }
    let mut gs_kept = 0usize;
    for s in first..=last {
        if let Some(at) = layout.size_word(s)
            && Recorded::unpack(host.meta(region, at).load()).kept
            && let Some(whole) = layout.slot_unit(s)
        {
            let (a, b) = (whole.start.max(unit.start), whole.end.min(unit.end));
            if a < b {
                gs_kept += dirty_pages(host, region, Unit { start: a, end: b }).0 * layout.page_size();
            }
        }
    }
""")
sub(R, """    release_range(host, region, first, last, low, high + 1);
    (Purge::Done(cleaned * page), read)""", """    if gs_kept != 0 {
        crate::gs::kept_add(-(gs_kept as i64));
        crate::gs::add(crate::gs::KEPT_PURGED_B, gs_kept);
    }
    for s in first..=last {
        if let Some(at) = layout.size_word(s) {
            let word = host.meta(region, at);
            let mut r = Recorded::unpack(word.load());
            r.tag = crate::gs::rule();
            r.when = crate::gs::fine() as u16;
            if r.kept && layout.slot_unit(s).is_some_and(|u| all_clean(host, region, u)) {
                r.kept = false;
            }
            word.store(r.pack());
        }
    }
    release_range(host, region, first, last, low, high + 1);
    (Purge::Done(cleaned * page), read)""")

# --- Class: the switch, the kept marks, the claims and frees counted.
C = "back/class.rs"
sub(C, """    fn keep_grown(&self, class: Class, charge: usize) -> bool {
        let budget = kept_grown(self.space.plan().geometry());
        // A slot that alone is past the budget is refused without being counted, so that a
        // large block's charge never sits in the count for a moment, where another keep or a
        // decay pass starting then would see it.
        if charge > budget {
            return false;
        }
        self.retain(class, charge);
        if self.retained_large() <= budget {
            return true;
        }
        self.unretain(class, charge);
        false
    }""", """    fn keep_grown(&self, class: Class, charge: usize) -> bool {
        self.gs_keep_grown(class, charge, false)
    }

    fn gs_keep_grown(&self, class: Class, charge: usize, freed: bool) -> bool {
        let cfg = crate::gs::config();
        let heap = if cfg.tallies() { self.gs_heap() } else { 0 };
        let budget = cfg.budget(kept_grown(self.space.plan().geometry()), heap);
        crate::gs::max(crate::gs::BUDGET_MAX, budget);
        crate::gs::max(crate::gs::HEAP_MAX, heap);
        crate::gs::max(crate::gs::RETAINED_LARGE_MAX, self.retained_large());
        // Modes 4 and 5: a grown block's free keeps its slot as any free does; a move's old
        // slot is kept within the budget, counted over kept grown slots (4) or all retained
        // large memory (5).
        let mode = match cfg.mode {
            crate::gs::FREE_OWN | crate::gs::FREE_MAIN if freed => crate::gs::KEEP,
            crate::gs::FREE_OWN => crate::gs::OWN,
            crate::gs::FREE_MAIN => crate::gs::MAIN,
            m => m,
        };
        match mode {
            // A grown block's free under modes 4 and 5 is a plain free: retained, and not in
            // the tally of kept grown slots (its caller does not mark it kept either).
            crate::gs::KEEP if mode != cfg.mode => {
                self.retain(class, charge);
                return true;
            }
            crate::gs::KEEP => {
                self.retain(class, charge);
                crate::gs::kept_add(charge as i64);
                return true;
            }
            crate::gs::NEVER => return false,
            _ => {}
        }
        // A slot that alone is past the budget is refused without being counted, so that a
        // large block's charge never sits in the count for a moment, where another keep or a
        // decay pass starting then would see it.
        if charge > budget {
            return false;
        }
        self.retain(class, charge);
        let kept = crate::gs::kept_add(charge as i64);
        let level = if mode == crate::gs::OWN {
            kept.min(self.retained_large())
        } else {
            self.retained_large()
        };
        if level <= budget {
            return true;
        }
        crate::gs::kept_add(-(charge as i64));
        self.unretain(class, charge);
        false
    }""")
# A grown block's free.
sub(C, """        if self.return_unwritten(host, region, slot, class) {
            return (0, false);
        }
""", """        if self.return_unwritten(host, region, slot, class) {
            return (0, false);
        }
        self.gs_res(host, region, slot);
""")
sub(C, """        if self.keep_grown(class, charge) {
            return (0, true);
        }
""", """        if self.gs_keep_grown(class, charge, true) {
            if !matches!(crate::gs::config().mode, crate::gs::FREE_OWN | crate::gs::FREE_MAIN) {
                self.gs_mark_kept(host, region, slot);
            }
            crate::gs::add(crate::gs::KEEP_FREE_N, 1);
            crate::gs::add(crate::gs::KEEP_FREE_B, charge);
            return (0, true);
        }
        crate::gs::add(crate::gs::REFUSE_FREE_N, 1);
        crate::gs::add(crate::gs::REFUSE_FREE_B, charge);
""")
sub(C, """        let purged = unsafe { region::purge_held(host, region, slot) };
        region::unadvise(host, region, slot);
""", """        let purged = unsafe { region::purge_held(host, region, slot) };
        if purged {
            self.gs_tag(host, region, slot, crate::gs::T_FREE);
            crate::gs::add(crate::gs::RET + crate::gs::T_FREE as usize, charge);
        }
        region::unadvise(host, region, slot);
""")
# An unwritten block's free.
sub(C, """        unsafe { region::purge_held(host, region, slot) }
    }

    /// Frees a large slot""", """        let done = unsafe { region::purge_held(host, region, slot) };
        if done {
            self.gs_tag(host, region, slot, crate::gs::T_ZERO);
            crate::gs::add(crate::gs::RET + crate::gs::T_ZERO as usize, class.size());
        }
        done
    }

    /// Frees a large slot""")
# Every large free: live bytes, and what it is charged.
sub(C, """        let (charge, resident) = self.purge_before_release(host, region, slot, class);
        let (word, bit) = position(slot);""", """        if let Some(r) = region::recorded(host, region, slot) {
            crate::gs::live_add(-(r.size as i64));
        }
        let (charge, resident) = self.purge_before_release(host, region, slot, class);
        crate::gs::add(crate::gs::FREES_LARGE, 1);
        crate::gs::add(crate::gs::FREE_CHARGE, charge);
        let (word, bit) = position(slot);""")
# Claims and carves.
sub(C, """                region::settle_slot(&host, taken.region, slot, size)
            };
            self.unretain(class, settled.resident);""", """                region::settle_slot(&host, taken.region, slot, size)
            };
            if small {
                crate::gs::add(crate::gs::REVIVE_SMALL, settled.revived);
            } else {
                self.gs_claim(&host, taken.region, slot, size, settled.resident, settled.under);
            }
            self.unretain(class, settled.resident);""")
sub(C, "            self.grow(if small { bytes } else { size });\n",
    "            if small {\n"
    "                crate::gs::carve_small(bytes);\n"
    "            } else {\n"
    "                crate::gs::add(crate::gs::CARVE_LARGE, size);\n"
    "            }\n"
    "            self.grow(if small { bytes } else { size });\n")
sub(C, """            let now = Recorded {
                size,
                unwritten: false,
                ..before
            };""", """            let now = Recorded {
                size,
                unwritten: false,
                kept: false,
                ..before
            };
            crate::gs::live_add(size as i64);""")
sub(C, "        self.grow(carved);\n        Filled {",
    "        crate::gs::carve_small(carved);\n        self.grow(carved);\n        Filled {")
# Resizes in place: live bytes, and a shrink's return.
sub(C, """                let old = before.size;
                region::record(&host, region, slot, Recorded { size, ..before });""",
    """                let old = before.size;
                region::record(&host, region, slot, Recorded { size, ..before });
                crate::gs::live_add(size as i64 - old as i64);""")
sub(C, """                    if purged {
                        // Those pages are not written now; a later grow clears the bits again""",
    """                    if purged {
                        crate::gs::add(crate::gs::RET + crate::gs::T_SHRINK as usize, len);
                        self.gs_tag(&host, region, slot, crate::gs::T_SHRINK);
                        // Those pages are not written now; a later grow clears the bits again""")
# Moves.
sub(C, """                let into = self.offset_in_slot(region, class, index, from.as_ptr());
                (class.size(), into, Some((host, region, index)))""",
    """                let into = self.offset_in_slot(region, class, index, from.as_ptr());
                if let Some(r) = region::recorded(&host, region, index) {
                    crate::gs::live_add(-(r.size as i64));
                }
                (class.size(), into, Some((host, region, index)))""")
sub(C, """        let give_back = |start: usize, end: usize| -> bool {
            if start >= end {
                return true;
            }
""", """        let give_back = |start: usize, end: usize| -> bool {
            if start >= end {
                return true;
            }
            crate::gs::add(crate::gs::RET + crate::gs::T_MOVE as usize, end - start);
""")
sub(C, """        let keep = match (&host, slot) {
            (Some((host, region, index)), Some(class)) => {
                !region::recorded(host, *region, *index).is_some_and(|r| r.unwritten)
                    && self.keep_grown(class, region::charge(host, *region, *index))
            }
            _ => false,
        };""", """        crate::gs::add(crate::gs::MOVES, 1);
        crate::gs::add(crate::gs::MOVE_BYTES, len);
        let keep = match (&host, slot) {
            (Some((host, region, index)), Some(class)) => {
                let charge = region::charge(host, *region, *index);
                let kept = !region::recorded(host, *region, *index).is_some_and(|r| r.unwritten)
                    && self.keep_grown(class, charge);
                if kept {
                    self.gs_mark_kept(host, *region, *index);
                    crate::gs::add(crate::gs::KEEP_MOVE_N, 1);
                    crate::gs::add(crate::gs::KEEP_MOVE_B, charge);
                } else {
                    crate::gs::add(crate::gs::REFUSE_MOVE_N, 1);
                    crate::gs::add(crate::gs::REFUSE_MOVE_B, charge);
                }
                kept
            }
            _ => false,
        };""")
sub(C, """                let charge = if all {
                    region::mark_purged(&host, region, index);
                    0
                } else {""", """                self.gs_tag(&host, region, index, crate::gs::T_MOVE);
                let charge = if all {
                    region::mark_purged(&host, region, index);
                    0
                } else {""")

HELPERS = r'''
/// Scratch (bench/probes/grown-slots/grown_edit.py). Not product code.
impl<'a, O: Os> Back<'a, O> {
    /// The heap in use, as far as cheap tallies tell: live large blocks, and small memory
    /// carved less what is free and resident.
    pub(super) fn gs_heap(&self) -> usize {
        let small_free = self.retained().saturating_sub(self.retained_large());
        crate::gs::live_large() + crate::gs::carved_small().saturating_sub(small_free)
    }

    fn gs_tag(&self, host: &ClassHost<'a, O>, region: RegionId, slot: u32, tag: u8) {
        if let Some(now) = region::recorded(host, region, slot) {
            let fine = self.gs_fine();
            crate::gs::set_fine(fine);
            region::record(host, region, slot, Recorded { tag, when: fine as u16, ..now });
        }
    }

    fn gs_mark_kept(&self, host: &ClassHost<'a, O>, region: RegionId, slot: u32) {
        if let Some(now) = region::recorded(host, region, slot) {
            region::record(host, region, slot, Recorded { kept: true, ..now });
        }
    }

    fn gs_claim(
        &self,
        host: &ClassHost<'a, O>,
        region: RegionId,
        slot: u32,
        size: usize,
        resident: usize,
        under: usize,
    ) {
        let Some(before) = region::recorded(host, region, slot) else {
            return;
        };
        let fine = self.gs_fine();
        crate::gs::set_fine(fine);
        if before.kept {
            crate::gs::kept_add(-(resident as i64));
            crate::gs::add(crate::gs::KEPT_REUSED_N, 1);
            crate::gs::add(crate::gs::KEPT_REUSED_B, resident);
        }
        crate::gs::claim(under * host.layout().page_size(), before.tag, before.when, fine, size);
    }

    /// With the switch's `r`: marks clean the pages of the extent of the large slot being
    /// freed that `mincore` finds not resident, 64 pages at a time, halving a part that is
    /// partly resident down to 8 pages.
    fn gs_res(&self, host: &ClassHost<'a, O>, region: RegionId, slot: u32) {
        if !crate::gs::config().res {
            return;
        }
        let Some(now) = region::recorded(host, region, slot) else {
            return;
        };
        if now.huge {
            return;
        }
        let layout = host.layout();
        let Some(unit) = layout.slot_unit(slot) else {
            return;
        };
        let end = region::extent_of(layout, slot, unit, now.size, false);
        let mut p = unit.start;
        while p < end {
            let to = ((p / 64 + 1) * 64).min(end);
            self.gs_res_range(host, region, slot, p, to, 3);
            p = to;
        }
    }

    fn gs_res_range(
        &self,
        host: &ClassHost<'a, O>,
        region: RegionId,
        slot: u32,
        from: u32,
        to: u32,
        depth: u32,
    ) {
        let layout = host.layout();
        let page = layout.page_size();
        let ptr = self.space.region(region).map_addr(|a| a.saturating_add(from as usize * page));
        let pages = (to - from) as usize;
        crate::gs::add(crate::gs::RES_CALLS, 1);
        // SAFETY: scratch. Whole pages of a slot the caller holds and is freeing; mincore
        // reads no memory.
        match unsafe { self.space.resident(ptr, pages * page) } {
            Ok(0) => {
                let at = from as usize * page - layout.slot_bytes(slot);
                region::mark_clean(host, region, slot, at, pages * page);
                crate::gs::add(crate::gs::RES_CLEANED, pages * page);
            }
            Ok(n) if n == pages => {}
            Ok(_) if depth > 0 && pages > 1 => {
                let mid = from + (to - from) / 2;
                self.gs_res_range(host, region, slot, from, mid, depth - 1);
                self.gs_res_range(host, region, slot, mid, to, depth - 1);
            }
            _ => {}
        }
    }
}
'''
p = SRC / C
p.write_text(p.read_text() + HELPERS)

# --- Purge: the rule each walk is doing, and what it returned.
P = "back/purge.rs"
sub(P, """        let Some(host) = self.host(class) else {
            return (0, 1, true, 0);
        };
        let layout = *host.layout();""", """        let Some(host) = self.host(class) else {
            return (0, 1, true, 0);
        };
        if !matches!(scope, Scope::All) {
            crate::gs::set_rule(crate::gs::T_PASS, self.gs_fine());
        }
        let layout = *host.layout();""")
sub(P, """        self.unretain(class, purged);
        if purged != 0 {
            self.state.walk.returned.fetch_add(purged, Relaxed);""", """        self.unretain(class, purged);
        crate::gs::ret(!class.is_small(), purged);
        if purged != 0 {
            self.state.walk.returned.fetch_add(purged, Relaxed);""")
sub(P, """        let debt = self.state.growth.swap(0, SeqCst);
        let owed = growth_goal(""", """        crate::gs::set_rule(crate::gs::T_GROWTH, self.gs_fine());
        let debt = self.state.growth.swap(0, SeqCst);
        let owed = growth_goal(""")
sub(P, """        let mut purged = 0;
        for region in (0..plan.objects()).rev().filter_map(|i| plan.region(i)) {""", """        let mut purged = 0;
        crate::gs::set_rule(crate::gs::T_TRIM, self.gs_fine());
        for region in (0..plan.objects()).rev().filter_map(|i| plan.region(i)) {""")
H = "back/hold.rs"
sub(H, """        let (bytes, read, done, _) =
            self.purge_region(class, region, budget, limit, true, Scope::All, false);""", """        crate::gs::set_rule(crate::gs::T_HOLD, self.state.hold.fine.count());
        let (bytes, read, done, _) =
            self.purge_region(class, region, budget, limit, true, Scope::All, false);""")
p = SRC / H
p.write_text(p.read_text() + r'''
/// Scratch (bench/probes/grown-slots/grown_edit.py). Not product code.
impl<O: Os> Back<'_, O> {
    pub(super) fn gs_fine(&self) -> u32 {
        self.state.hold.fine.count()
    }
}
''')
print("grown-slots switch applied to", TREE)
