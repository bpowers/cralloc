import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Experiment, after freecap.py (whose release stamps and scope `Old` it uses; its free-side
# payment is switched off here): free large slots go back once their region has had no release
# for OLD_TICKS (2^28 ticks, about 60 ms; largeidle2.py makes it about 250 ms). A heartbeat
# that finds 2^25 ticks (about 8 ms) gone since the last such pass walks the heap once, within
# the growth payment's bound, purging the free units of large-class regions released into no
# more recently than that. Small classes are left to the clocked pass.
P = "crates/asmalloc/src/back/purge.rs"
sub(P, """        if bytes == 0 {
            return;
        }
        let geometry = self.space.plan().geometry();
        let step = growth_step(geometry);""", """        if true || bytes == 0 {
            return;
        }
        let geometry = self.space.plan().geometry();
        let step = growth_step(geometry);""")
sub(P, """        let (purged, read, done) = match self.purgeable(region).filter(|_| !recent) {""",
    """        let (purged, read, done) = match self
            .purgeable(region)
            .filter(|c| !recent && !(scope == Scope::Old && c.is_small()))
        {""")
sub(P, """    pub fn heartbeat_with(&self, now: &dyn Fn() -> u64) {
        let _ = self.decay_step(now);
    }""", """    pub fn heartbeat_with(&self, now: &dyn Fn() -> u64) {
        let _ = self.decay_step(now);
        self.large_idle_step(now);
    }

    /// Purges the free units of large-class regions released into no more recently than
    /// `OLD_TICKS`, one bounded circuit at most every 2^25 ticks.
    fn large_idle_step(&self, now: &dyn Fn() -> u64) {
        static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let t = now();
        let last = LAST.load(Relaxed);
        if t.wrapping_sub(last) < 1 << 25 || LAST.compare_exchange(last, t, Relaxed, Relaxed).is_err() {
            return;
        }
        let regions = self.walk_regions().max(1);
        let (mut spent, mut finished) = (0, 0);
        while spent < PURGE_CAP_WORDS && finished < regions {
            let (read, _, done) = self.purge_step(PURGE_CAP_WORDS - spent, usize::MAX, Scope::Old);
            spent += read;
            finished += usize::from(done);
        }
    }""")
