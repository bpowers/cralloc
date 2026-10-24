#!/usr/bin/env python3
"""Turns a scratch copy of the repository into the account build for Meilisearch's peak
(docs/research/meili-peak.md). Never product code: apply it to a copy made by build.sh.

  peak_edit.py TREE

The allocator gets `asmalloc::diag` (diag.rs here): counters of what each rule returned, what
large claims will fault and why, what large frees are charged against what is resident, growth
carved, in place and paid, and each region's last purge. bench-alloc's memstat sampler is
replaced by memstat_peak.rs, which writes a breakdown of resident memory every
ASMALLOC_MEMSTAT_MS with the counters, the process's anonymous memory and glibc's arenas;
its je.rs by je_peak.rs, which does the same from jemalloc's statistics. Meilisearch's driver
gets the two features (`asmalloc-memstat`, `je-stats`).
"""
import shutil
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
HERE = Path(__file__).resolve().parent
SRC = TREE / "crates/asmalloc/src"


def sub(path, old, new, count=1):
    p = TREE / path
    s = p.read_text()
    if s.count(old) != count:
        raise SystemExit(f"{path}: {s.count(old)} matches (want {count}) for {old[:80]!r}")
    p.write_text(s.replace(old, new))


A = "crates/asmalloc/src/"
shutil.copy(HERE / "diag.rs", SRC / "diag.rs")
sub(A + "lib.rs", "mod heap;\n", "mod heap;\npub mod diag;\n")

# --- Settled learns the clean pages under what was claimed.
R = A + "back/region.rs"
sub(R, "    pub(crate) revived: usize,\n}", "    pub(crate) revived: usize,\n    /// Diagnostic: clean pages under the claim.\n    pub(crate) under: usize,\n}")
sub(R, "        revived: (cleaned * page).saturating_sub(claimed),\n",
    "        revived: (cleaned * page).saturating_sub(claimed),\n        under: cleaned,\n")
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
# Growth in place: the clean pages a widening covers.
sub(R, """        let word = host.meta(region, layout.clean_word(w));
        if word.load() & inside != 0 {
            word.fetch_and(!inside);
        }""", """        let word = host.meta(region, layout.clean_word(w));
        if word.load() & inside != 0 {
            let old = word.fetch_and(!inside);
            crate::diag::add(crate::diag::COVER, (old & inside).count_ones() as usize * layout.page_size());
        }""")

# --- Claims, carves and frees.
C = A + "back/class.rs"
sub(C, """            let settled = region::settle(&host, taken.region, taken.word, taken.bits);
            resident += settled.resident;""", """            let settled = region::settle(&host, taken.region, taken.word, taken.bits);
            self.diag_revive(class.class(), taken.region, settled.under);
            resident += settled.resident;""")
sub(C, "        self.grow(carved);\n        Filled {",
    "        crate::diag::add(crate::diag::CARVE_SMALL, carved);\n"
    "        crate::diag::add(crate::diag::CLASS_SMALL_CARVE + class.class().index() as usize, carved);\n"
    "        self.grow(carved);\n        Filled {")
sub(C, """        let small = class.is_small();
        let (region, slot, zeroed) = if let Some(taken) = tree::claim(&host, 1, !small) {""",
    """        let small = class.is_small();
        let mut diag_under = None;
        let (region, slot, zeroed) = if let Some(taken) = tree::claim(&host, 1, !small) {""")
sub(C, """                region::settle_slot(&host, taken.region, slot, size)
            };
            self.unretain(class, settled.resident);""", """                region::settle_slot(&host, taken.region, slot, size)
            };
            if small {
                self.diag_revive(class, taken.region, settled.under);
            } else {
                diag_under = Some(settled.under);
            }
            self.unretain(class, settled.resident);""")
sub(C, "            self.grow(if small { bytes } else { size });\n",
    "            if small {\n"
    "                crate::diag::add(crate::diag::CARVE_SMALL, bytes);\n"
    "            } else {\n"
    "                crate::diag::add(crate::diag::CARVE_LARGE, size);\n"
    "                crate::diag::add(crate::diag::LARGE_CARVES, 1);\n"
    "                crate::diag::add(crate::diag::CLASS_CARVE + class.index() as usize, size);\n"
    "            }\n"
    "            self.grow(if small { bytes } else { size });\n")
sub(C, "        let offset = layout.slot_bytes(slot) + into;\n",
    "        let offset = layout.slot_bytes(slot) + into;\n"
    "        if let Some(under) = diag_under {\n"
    "            self.diag_claim(class, region, offset, size, under);\n"
    "        }\n")
sub(C, """        let (charge, resident) = self.purge_before_release(host, region, slot, class);
        let (word, bit) = position(slot);""", """        let (charge, resident) = self.purge_before_release(host, region, slot, class);
        if resident {
            self.diag_free(class, region, slot, charge);
        }
        let (word, bit) = position(slot);""")
sub(C, "        let purged = unsafe { region::purge_held(host, region, slot) };\n",
    "        let purged = unsafe { region::purge_held(host, region, slot) };\n"
    "        if purged {\n"
    "            crate::diag::add(crate::diag::RET_FREE_ADVISED, charge);\n"
    "            self.diag_rule(region, crate::diag::R_SELF);\n"
    "        }\n")
sub(C, """        if self.retained_large() <= budget {
            return true;
        }
        self.unretain(class, charge);
        false""", """        if self.retained_large() <= budget {
            crate::diag::add(crate::diag::KEPT, charge);
            return true;
        }
        self.unretain(class, charge);
        crate::diag::add(crate::diag::KEPT_REFUSED, charge);
        false""")
sub(C, """        let give_back = |start: usize, end: usize| -> bool {
            if start >= end {
                return true;
            }
""", """        let give_back = |start: usize, end: usize| -> bool {
            if start >= end {
                return true;
            }
            crate::diag::add(crate::diag::RET_MOVE, end - start);
            if let Some((_, region, _)) = &host {
                self.diag_rule(*region, crate::diag::R_SELF);
            }
""")
sub(C, "        let page = self.space.plan().geometry().page_size();\n        let mut dest = if zeroed { Dest::Zero } else { Dest::Dirty };\n",
    "        let page = self.space.plan().geometry().page_size();\n        let mut dest = if zeroed { Dest::Zero } else { Dest::Dirty };\n"
    "        crate::diag::add(crate::diag::MOVES, 1);\n        crate::diag::add(crate::diag::MOVE_BYTES, len);\n")
sub(C, """                    if purged {
                        // Those pages are not written now; a later grow clears the bits again""",
    """                    if purged {
                        crate::diag::add(crate::diag::RET_SHRINK, len);
                        self.diag_rule(region, crate::diag::R_SELF);
                        // Those pages are not written now; a later grow clears the bits again""")

# --- The walk's rules.
P = A + "back/purge.rs"
sub(P, """            Some(class) => self.purge_region(class, region, budget, limit, true, scope, counting),
            None => (0, 1, true, 0),
        };
""", """            Some(class) => self.purge_region(class, region, budget, limit, true, scope, counting),
            None => (0, 1, true, 0),
        };
        if purged != 0 {
            self.diag_rule(region, if scope == Scope::All { crate::diag::R_GROWTH } else { crate::diag::R_PASS });
        }
""")
sub(P, "            spent += read;\n            *returned += purged;\n",
    "            spent += read;\n            *returned += purged;\n            crate::diag::add(crate::diag::RET_PASS, purged);\n")
sub(P, """        let goal = owed.min(PURGE_STEP_PAGES as usize * geometry.page_size());
""", """        let goal = owed.min(PURGE_STEP_PAGES as usize * geometry.page_size());
        crate::diag::add(crate::diag::PAYMENTS, 1);
        crate::diag::add(crate::diag::DEBT_TAKEN, debt);
        crate::diag::add(crate::diag::DEBT_OWED, owed);
        crate::diag::add(crate::diag::DEBT_GOAL, goal);
        crate::diag::add(crate::diag::DEBT_UNDER_CAP, debt - owed);
        if owed == 0 {
            crate::diag::add(crate::diag::PAY_UNDER_CAP, 1);
        }
""")
sub(P, """        if owed > goal && purged != 0 {
            self.state.growth.fetch_add(owed - goal, SeqCst);
        }
        purged
""", """        if owed > goal && purged != 0 {
            self.state.growth.fetch_add(owed - goal, SeqCst);
            crate::diag::add(crate::diag::DEBT_CARRIED, owed - goal);
        } else if owed > goal {
            crate::diag::add(crate::diag::DEBT_DRY, owed - goal);
        }
        crate::diag::add(crate::diag::RET_GROWTH, purged);
        purged
""")
sub(P, """                // What a payment under the cap does: takes the debt and returns nothing.
                self.state.growth.swap(0, SeqCst);""", """                // What a payment under the cap does: takes the debt and returns nothing.
                let dropped = self.state.growth.swap(0, SeqCst);
                crate::diag::add(crate::diag::DEBT_UNDER_CAP, dropped);""")
sub(P, "                    .0;\n            }\n        }\n        purged\n",
    "                    .0;\n            }\n        }\n        crate::diag::add(crate::diag::RET_TRIM, purged);\n        purged\n")
sub(P, "        self.unretain(class, purged);\n        if purged != 0 {\n            self.state.walk.returned.fetch_add(purged, Relaxed);\n",
    "        self.unretain(class, purged);\n        crate::diag::add(crate::diag::CLASS_RET + class.index() as usize, purged);\n"
    "        if purged != 0 {\n            self.state.walk.returned.fetch_add(purged, Relaxed);\n")

# --- The hold.
H = A + "back/hold.rs"
sub(H, """        PassStep {
            more: stopped && purged != 0 && purged < over,
            returned: purged,
        }""", """        crate::diag::add(crate::diag::HOLD_LOOKS, 1);
        crate::diag::add(crate::diag::RET_HOLD, purged);
        PassStep {
            more: stopped && purged != 0 && purged < over,
            returned: purged,
        }""")
sub(H, "        if done && stamp & WALKED_FROM_TOP != 0 {\n",
    "        if bytes != 0 {\n            self.diag_rule(region, crate::diag::R_HOLD);\n        }\n"
    "        if done && stamp & WALKED_FROM_TOP != 0 {\n")
HELPERS = r'''
/// Scratch diagnostics for bench/probes/meili-peak. Not product code.
impl<O: Os> Back<'_, O> {
    pub(super) fn diag_rule(&self, region: RegionId, rule: u8) {
        crate::diag::rule(region.index(), rule, self.state.hold.fine.count());
    }

    pub(super) fn diag_inspect(&self) {
        let large = self.retained_large();
        crate::diag::set(crate::diag::RETAINED_LARGE, large);
        crate::diag::set(crate::diag::RETAINED_SMALL, self.retained().saturating_sub(large));
        crate::diag::set(crate::diag::HOLD_FINE, self.state.hold.fine.count() as usize);
        crate::diag::set(crate::diag::DEBT_NOW, self.state.growth.load(SeqCst));
    }

    pub(super) fn diag_region_age(&self, host: &ClassHost<'_, O>, region: RegionId) {
        let Some(at) = host.layout().hold_word() else {
            return;
        };
        let stamp = host.meta(region, at).load();
        let age = if stamp == 0 {
            u32::MAX
        } else {
            self.state.hold.fine.count().wrapping_sub(stamp as u32)
        };
        if region.index() < crate::diag::REGIONS {
            crate::diag::REGION_AGE[region.index()].store(age, Relaxed);
        }
    }

    /// Purged pages a small claim took back, by the rule that purged its region.
    pub(super) fn diag_revive(&self, class: crate::classes::Class, region: RegionId, under: usize) {
        if under == 0 {
            return;
        }
        let bytes = under * self.space.plan().geometry().page_size();
        let index = region.index().min(crate::diag::REGIONS - 1);
        let rule = crate::diag::REGION_RULE[index].load(Relaxed) as usize;
        crate::diag::add(crate::diag::SMALL_REVIVE, bytes);
        crate::diag::add(crate::diag::SMALL_REVIVE_BY + rule, bytes);
        crate::diag::add(crate::diag::CLASS_SMALL_REVIVE + class.index() as usize, bytes);
    }

    /// A large slot just claimed for a block of `size` bytes, `offset` bytes into `region`,
    /// whose claim found `under` clean pages under the block: what it will fault, and why.
    pub(super) fn diag_claim(
        &self,
        class: crate::classes::Class,
        region: RegionId,
        offset: usize,
        size: usize,
        under: usize,
    ) {
        use crate::diag::*;
        let page = self.space.plan().geometry().page_size();
        let from = offset & !(page - 1);
        let to = (offset + size.max(1)).next_multiple_of(page);
        let start = self.space.region(region).map_addr(|a| a.saturating_add(from));
        // SAFETY: scratch diagnostic. The caller holds the slot, and nothing writes it before
        // the claim returns; mincore reads no memory.
        let res = unsafe { self.space.resident(start, to - from) }.map_or(to - from, |p| p * page);
        let nonres = (to - from).saturating_sub(res);
        let index = region.index().min(REGIONS - 1);
        let (rule, fine) = (REGION_RULE[index].load(Relaxed), REGION_FINE[index].load(Relaxed));
        let clean = under * page;
        let ci = class.index() as usize;
        add(LARGE_CLAIMS, 1);
        add(LARGE_CLAIM_BYTES, size);
        add(LARGE_CLAIM_CLEAN, clean);
        add(LARGE_CLAIM_NONRES, nonres);
        add(CLASS_CLAIM + ci, size);
        add(CLASS_FAULT + ci, nonres);
        // What the clean bits marked goes to the region's last purge (bucket 1 of no rule: a
        // region never purged, so pages past an earlier tenant's extent); what they did not
        // mark is bucket 0 of no rule: pages a tenant was given and never wrote.
        let marked = clean.min(nonres);
        if rule == R_NONE {
            add(LARGE_FAULT_BY + 1, marked);
        } else {
            let age = age_bucket(self.state.hold.fine.count().wrapping_sub(fine));
            add(LARGE_FAULT_BY + rule as usize * AGES + age, marked);
        }
        add(LARGE_FAULT_BY, nonres - marked);
    }

    /// A large slot about to be released resident with `charge` bytes counted.
    pub(super) fn diag_free(&self, class: crate::classes::Class, region: RegionId, slot: u32, charge: usize) {
        use crate::diag::*;
        let page = self.space.plan().geometry().page_size();
        let len = class.size().min(1 << 30);
        let start = self.space.slot(region, class, slot);
        // SAFETY: scratch diagnostic. The caller holds the slot it is freeing; mincore reads
        // no memory.
        let res = unsafe { self.space.resident(start, len) }.map_or(0, |p| p * page);
        let ci = class.index() as usize;
        add(LARGE_FREES, 1);
        add(LARGE_FREE_CHARGE, charge);
        add(LARGE_FREE_RES, res);
        add(CLASS_FREE_CHARGE + ci, charge);
        add(CLASS_FREE_RES + ci, res);
    }
}
'''
p = TREE / H
p.write_text(p.read_text() + HELPERS)

I = A + "back/inspect.rs"
sub(I, "        visitor.retained(self.retained());\n",
    "        visitor.retained(self.retained());\n        self.diag_inspect();\n")
sub(I, "        let base = self.space.region(region).as_ptr().cast_const();\n        visitor.region(&Region {",
    "        let base = self.space.region(region).as_ptr().cast_const();\n"
    "        if !class.is_small() {\n            self.diag_region_age(&host, region);\n        }\n"
    "        visitor.region(&Region {")

# --- bench-alloc: the samplers, and the features that select them.
shutil.copy(HERE / "memstat_peak.rs", TREE / "bench/alloc/src/memstat.rs")
shutil.copy(HERE / "je_peak.rs", TREE / "bench/alloc/src/je.rs")
sub("bench/alloc/Cargo.toml", 'je = ["dep:tikv-jemallocator"]\n',
    'je = ["dep:tikv-jemallocator"]\n'
    '# tikv-jemallocator with statistics, counted, and sampled (meili-peak). Not an allocator of its own.\n'
    'je-stats = ["je", "tikv-jemallocator/stats"]\n')
sub("bench/alloc/build.rs", '["asmalloc-memstat", "asmalloc-background", "asmalloc-domains", "live-bytes", "memacct"]',
    '["asmalloc-memstat", "asmalloc-background", "asmalloc-domains", "live-bytes", "memacct", "je-stats"]')
sub("bench/alloc/src/lib.rs", """#[cfg(feature = "je")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;""", """#[cfg(all(feature = "je", not(feature = "je-stats")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
#[cfg(feature = "je-stats")]
#[global_allocator]
static GLOBAL: je::Counting = je::Counting;""")
sub("bench/apps/meilisearch/Cargo.toml", 'je = ["bench-alloc/je"]\n',
    'je = ["bench-alloc/je"]\n'
    'je-stats = ["bench-alloc/je-stats"]\n'
    'asmalloc-memstat = ["bench-alloc/asmalloc-memstat"]\n')
print("meili-peak edits applied to", TREE)
