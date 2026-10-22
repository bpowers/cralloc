import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic, on main: free large memory held on credit, the payment taking free memory wherever
# its walk finds it (`docs/research/hold-credit.md`, "The second diagnostic").
#
# - The hold returns a large region's free slots only once the region has gone HOLD_CEIL fine
#   steps (default 1024, about 2 s) without a free that left a slot resident, instead of 64.
# - Resident growth is a carve, as on main, or a claim of a large slot whose pages under the
#   block were returned: those pages fault back, and are charged to the growth debt as a carve
#   is. A small class's claim of returned pages is still not charged.
# - A payment returns all it owes, not `PURGE_STEP_PAGES` (a stand-in for the background
#   thread, which pays in the real build), in two passes over the heap from the shared walk:
#   the first takes only large-class regions not freed into within the base hold (64 steps),
#   the second, only if debt remains, any region, as main's payment does. Each pass reads at most
#   `PURGE_CAP_WORDS` words of bitmap and goes at most once round.
# - What a payment could not take stays owed, whatever its walk found, for the heartbeats to
#   pay (main drops it when the walk finds nothing). Debt under the cap is dropped, as on main:
#   `growth_goal` owes only what is over the cap.
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
sub(P, """        let goal = owed.min(PURGE_STEP_PAGES as usize * geometry.page_size());
""", """        let goal = owed;
""")
sub(P, """        let regions = self.walk_regions().max(1);
        let (mut spent, mut finished, mut purged) = (0, 0, 0);
        while purged < goal && spent < PURGE_CAP_WORDS && finished < regions {
            let (read, bytes, done) =
                self.purge_step(PURGE_CAP_WORDS - spent, goal - purged, Scope::All);
            spent += read;
            purged += bytes;
            finished += usize::from(done);
        }
""", """        let regions = self.walk_regions().max(1);
        let (mut spent, mut finished, mut purged) = (0, 0, 0);
        // CREDIT2 PASS ONE
        while purged < goal && spent < PURGE_CAP_WORDS && finished < regions {
            let (read, bytes, done) =
                self.purge_step_aged(PURGE_CAP_WORDS - spent, goal - purged, Scope::All, true);
            spent += read;
            purged += bytes;
            finished += usize::from(done);
        }
        let first = purged;
        let (mut spent2, mut finished2) = (0, 0);
        // CREDIT2 PASS TWO
        while purged < goal && spent2 < PURGE_CAP_WORDS && finished2 < regions {
            let (read, bytes, done) =
                self.purge_step_aged(PURGE_CAP_WORDS - spent2, goal - purged, Scope::All, false);
            spent2 += read;
            purged += bytes;
            finished2 += usize::from(done);
        }
        let _ = first;
""")
sub(P, """        if owed > goal && purged != 0 {
            self.state.growth.fetch_add(owed - goal, Relaxed);
        }
""", """        if owed > purged {
            self.state.growth.fetch_add(owed - purged, Relaxed);
        }
        // CREDIT2 PAY END
""")
sub(P, """    fn purge_step(&self, budget: usize, limit: usize, scope: Scope) -> (usize, usize, bool) {
        let plan""", """    fn purge_step(&self, budget: usize, limit: usize, scope: Scope) -> (usize, usize, bool) {
        self.purge_step_aged(budget, limit, scope, false)
    }

    /// `purge_step`, passing over (as done) every region that is not a large class's aged
    /// past the base hold if `aged`.
    fn purge_step_aged(
        &self,
        budget: usize,
        limit: usize,
        scope: Scope,
        aged: bool,
    ) -> (usize, usize, bool) {
        let plan""")
sub(P, """        let (purged, read, done) = match self.purgeable(region) {
            Some(class) => self.purge_region(class, region, budget, limit, true, scope),
            None => (0, 1, true),
        };""", """        let (purged, read, done) = match self.purgeable(region) {
            Some(class) if !aged || self.hold_aged(class, region) => {
                self.purge_region(class, region, budget, limit, true, scope)
            }
            _ => (0, 1, true),
        };""")

H = "crates/asmalloc/src/back/hold.rs"
sub(H, """        if stamp == 0 || !held_for(stamp, count, reach.steps) {""",
    f"""        let steps = if reach.steps >= HOLD_STEPS {{ {ceil} }} else {{ reach.steps }};
        if stamp == 0 || !held_for(stamp, count, steps) {{""")
sub(H, """    /// A heartbeat's part of the hold: advances""", """    /// Whether `region`, of `class`, is a large class's region that has gone the base hold
    /// without a free that left a slot resident (or has no stamp): the first pass of a growth
    /// payment takes only these.
    pub(super) fn hold_aged(&self, class: crate::classes::Class, region: RegionId) -> bool {
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
        stamp == 0 || held_for(stamp, self.state.hold.fine.count(), HOLD_STEPS)
    }

    /// A heartbeat's part of the hold: advances""")
