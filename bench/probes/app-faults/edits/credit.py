import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic, on main: free large memory held on credit. The hold returns a region's free slots
# only past a ceiling of HOLD_CEIL fine steps (default 1024, about 2 s); resident growth, a carve
# or a claim that faults back returned pages, owes as much, paid at once and unbounded (a
# stand-in for the background thread) from free memory elsewhere, but never from a large-class
# region freed into within the base hold.
ceil = int(os.environ.get("HOLD_CEIL", "1024"))
R = "crates/asmalloc/src/back/region.rs"
sub(R, """    pub(crate) revived: usize,
}""", """    pub(crate) revived: usize,
    /// Bytes under a large block's claim that were returned to the OS: what it faults back.
    pub(crate) refaulted: usize,
}""")
sub(R, """        revived: (cleaned * page).saturating_sub(claimed),
    }""", """        revived: (cleaned * page).saturating_sub(claimed),
        refaulted: 0,
    }""")
sub(R, """            resident: layout.size(),
            revived: 0,
        };""", """            resident: layout.size(),
            revived: 0,
            refaulted: 0,
        };""")
sub(R, """    let end = extent_of(layout, slot, unit, size, false);
    let mut clean = 0usize;
""", """    let end = extent_of(layout, slot, unit, size, false);
    let mut clean = 0usize;
    let mut under = 0usize;
""")
sub(R, """        let inside = mask & below(w, end);
        if seen & inside != 0 {
            word.fetch_and(!inside);
        }
    }
    Settled {
        zeroed,
        resident: layout.size() - clean * layout.page_size(),
        revived: 0,
    }""", """        let inside = mask & below(w, end);
        under += count(seen & inside);
        if seen & inside != 0 {
            word.fetch_and(!inside);
        }
    }
    Settled {
        zeroed,
        resident: layout.size() - clean * layout.page_size(),
        revived: 0,
        refaulted: under * layout.page_size(),
    }""")
C = "crates/asmalloc/src/back/class.rs"
sub(C, """            self.unretain(class, settled.resident);
            self.retain(class, settled.revived);
            (taken.region, slot, settled.zeroed != 0)""", """            self.unretain(class, settled.resident);
            self.retain(class, settled.revived);
            // A claim that faults back returned pages grows resident memory as a carve does.
            self.grow(settled.refaulted);
            (taken.region, slot, settled.zeroed != 0)""")
P = "crates/asmalloc/src/back/purge.rs"
sub(P, "        let goal = owed.min(PURGE_STEP_PAGES as usize * geometry.page_size());\n",
    "        let goal = owed;\n")
# What a payment could not take stays owed, whatever its walk found: memory younger than the
# base hold is paid for once it ages, by the heartbeats that pay what is owed.
sub(P, "        if owed > goal && purged != 0 {\n            self.state.growth.fetch_add(owed - goal, Relaxed);\n        }\n",
    "        if owed > purged {\n            self.state.growth.fetch_add(owed - purged, Relaxed);\n        }\n")
sub(P, """        let (purged, read, done) = match self.purgeable(region) {
            Some(class) => self.purge_region(class, region, budget, limit, true, scope),
            None => (0, 1, true),
        };
        if done
            && self""", """        let (purged, read, done) = match self.purgeable(region) {
            Some(class) if !self.hold_young(class, region) => {
                self.purge_region(class, region, budget, limit, true, scope)
            }
            _ => (0, 1, true),
        };
        if done
            && self""")
H = "crates/asmalloc/src/back/hold.rs"
sub(H, """        if stamp == 0 || !held_for(stamp, count, reach.steps) {""",
    f"""        let steps = if reach.steps >= HOLD_STEPS {{ {ceil} }} else {{ reach.steps }};
        if stamp == 0 || !held_for(stamp, count, steps) {{""")
sub(H, """    /// A heartbeat's part of the hold: advances""", """    /// Whether `region`, of `class`, was freed into within the base hold: its free memory is
    /// not paid for growth.
    pub(super) fn hold_young(&self, class: crate::classes::Class, region: RegionId) -> bool {
        if class.is_small() {
            return false;
        }
        let Some(host) = self.host(class) else {
            return false;
        };
        let Some(at) = host.layout().hold_word() else {
            return false;
        };
        let stamp = host.meta(region, at).load();
        stamp != 0 && !held_for(stamp, self.state.hold.fine.count(), HOLD_STEPS)
    }

    /// A heartbeat's part of the hold: advances""")
