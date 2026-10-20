import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Experiment, not for merging: the retained cap enforced where memory is freed. A free or
# launder that adds to the retained count takes a free debt; once the debt is a growth step
# (2 MiB), the freer purges what retained memory is over the cap, within the growth payment's
# bound of 16,384 bitmap words a call, in scope FREE_SCOPE. Every free and launder stamps the
# regions it releases into with the time, so a scope `Old` can skip regions released into
# within OLD_TICKS (2^28 ticks, about 60 ms). freeold.py and growold.py switch scopes.
P = "crates/asmalloc/src/back/purge.rs"
sub(P, """    /// Every free unit: what growth over the cap and `trim` take.
    All,
}
""", """    /// Every free unit: what growth over the cap and `trim` take.
    All,
    /// Every free unit of a region nothing was freed into within `OLD_TICKS`.
    Old,
}

/// How recent a release keeps a region from a purge in scope `Old`: 2^28 ticks, about 60 ms.
const OLD_TICKS: u64 = 1 << 28;

/// The scope the free side's purge walks in.
const FREE_SCOPE: Scope = Scope::All;

/// The time of the last release into each region (by index), for scope `Old`.
pub(crate) static RELEASED: [core::sync::atomic::AtomicU64; 4096] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 4096];

/// The free side's debt: bytes added to the retained count since the last payment.
static FREE_DEBT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
""")
sub(P, "            Scope::All => 0,\n", "            Scope::All | Scope::Old => 0,\n")
sub(P, """        let (purged, read, done) = match self.purgeable(region) {
            Some(class) => self.purge_region(class, region, budget, limit, true, scope),""",
    """        let recent = scope == Scope::Old
            && RELEASED.get(region.index()).is_some_and(|t| {
                self.space.os().ticks().wrapping_sub(t.load(Relaxed)) < OLD_TICKS
            });
        let (purged, read, done) = match self.purgeable(region).filter(|_| !recent) {
            Some(class) => self.purge_region(class, region, budget, limit, true, scope),""")
sub(P, """    /// The regions the walk goes round: those below the highest ever given to a class.""",
    """    /// Stamps the region holding `ptr` as released into now.
    pub(super) fn stamp_release(&self, ptr: *const u8) {
        if let Some(location) = self.space.locate(ptr)
            && let Some(word) = RELEASED.get(location.region.index())
        {
            word.store(self.space.os().ticks(), Relaxed);
        }
    }

    /// The free side's payment: `bytes` were just added to the retained count by a free or a
    /// launder. Once the debt is a step, purges what is over the cap, within the bound.
    pub(super) fn pay_free(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let geometry = self.space.plan().geometry();
        let step = growth_step(geometry);
        if FREE_DEBT.fetch_add(bytes, Relaxed).wrapping_add(bytes) < step {
            return;
        }
        FREE_DEBT.store(0, Relaxed);
        let goal = self.retained().saturating_sub(retained_cap(geometry));
        let regions = self.walk_regions().max(1);
        let (mut spent, mut finished, mut purged) = (0, 0, 0);
        while purged < goal && spent < PURGE_CAP_WORDS && finished < regions {
            let (read, bytes, done) =
                self.purge_step(PURGE_CAP_WORDS - spent, goal - purged, FREE_SCOPE);
            spent += read;
            purged += bytes;
            finished += usize::from(done);
        }
    }

    /// The regions the walk goes round: those below the highest ever given to a class.""")
C = "crates/asmalloc/src/back/class.rs"
sub(C, """            tree::release_one(&host, region, word, bit);
            if count {
                self.retain(class, class.size());
            }
            return;
        }""", """            tree::release_one(&host, region, word, bit);
            self.stamp_release(first.address());
            if count {
                self.retain(class, class.size());
                self.pay_free(class.size());
            }
            return;
        }""")
sub(C, """        release.finish();
        if count {
            self.retain(class, objects.len().saturating_mul(class.size()));
        }""", """        release.finish();
        let mut last = usize::MAX;
        for object in objects {
            let region = (object.address().addr().wrapping_sub(self.space.base().as_ptr().addr()))
                >> self.space.plan().region_shift();
            if region != last {
                self.stamp_release(object.address());
                last = region;
            }
        }
        if count {
            self.retain(class, objects.len().saturating_mul(class.size()));
            self.pay_free(objects.len().saturating_mul(class.size()));
        }""")
sub(C, """        tree::release_one(host, region, word, bit);
        self.retain(class, charge);
    }""", """        tree::release_one(host, region, word, bit);
        if let Some(stamp) = super::purge::RELEASED.get(region.index()) {
            stamp.store(self.space.os().ticks(), Relaxed);
        }
        self.retain(class, charge);
        self.pay_free(charge);
    }""")
