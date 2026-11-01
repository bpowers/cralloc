//! `collect`: all of the middle tier's maintenance at once, without waiting for the clock.
//!
//! 1. The caller's own slab is emptied through its own routines, with no fence: each class's
//!    loaded magazine is taken with store-if-equal, as the age limit takes one (`age.rs`), and
//!    each spare stack is popped empty.
//! 2. Every other slab a slow path has run on is drained with the drain protocol (`drain.rs`):
//!    a claim, markers and one fence per CPU. Drained magazines go to the depot.
//! 3. Every class's depot stack, in every cache domain and the shared pool, is laundered, a
//!    magazine at a time.
//!
//! The back end's purge is the heap's to ask for afterwards. What collect empties stays empty
//! only while nothing allocates: it is for a program that knows it is idle, and it is a racy
//! sweep beside threads that are not. A thread that moves to another CPU during step 1 empties
//! some of that CPU's slab instead and leaves some of its first CPU's to step 2. A slab whose
//! drain backs out or finds another drainer keeps its magazines, and with drains off (no
//! `membarrier`) only the caller's own slab is emptied. Each loop is bounded: by the classes,
//! by the depth a spare stack had when its emptying began (stock can make it hundreds deep), and
//! by the objects a depot stack held when its launder began, so concurrent pushes cannot keep
//! it going.

use super::{Cache, Pool, Source};
use crate::classes::SmallClass;
use crate::front::{Cpu, Routines, Spare};
use crate::os::Os;

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// Sends everything cached in slabs and the depot back to the source, as far as it can
    /// (see the module documentation).
    pub fn collect(&self) {
        if !self.prepare() {
            // The magazine area cannot be used, so no magazine exists.
            return;
        }
        let me = self.routines.cpu();
        if let Some(cpu) = me {
            self.empty_own(cpu);
        }
        let count = self
            .depot
            .common
            .cpus
            .load(core::sync::atomic::Ordering::Acquire);
        for index in 0..count {
            if let Some(cpu) = self.routines.slabs().cpu(index as i32)
                && Some(cpu) != me
            {
                self.drain(cpu);
            }
        }
        for pool in self.depot.pools() {
            for class in SmallClass::all() {
                self.launder_stack(pool, class);
            }
        }
    }

    /// Empties the slab of `cpu`, the CPU the thread is on, through the routines.
    fn empty_own(&self, cpu: Cpu) {
        for class in SmallClass::all() {
            if let Some(seen) = self.hot_magazine(cpu, class)
                && self.reclaim(class, seen)
            {
                self.slab(cpu)
                    .stamp(class)
                    .store(0, core::sync::atomic::Ordering::Relaxed);
            }
            let mut stale = false;
            for kind in [Spare::Full, Spare::Empty] {
                let depth = self.spare_depth(class, kind).unwrap_or(0);
                for _ in 0..depth {
                    let Some(magazine) = self.spare_pop(class, kind, &mut stale) else {
                        break;
                    };
                    self.launder_held(class, &magazine);
                    self.to_depot(class, magazine);
                }
            }
        }
    }

    /// Launders `class`'s depot stack in `pool`, up to the objects it held when this began.
    fn launder_stack(&self, pool: Pool, class: SmallClass) {
        let held = self
            .depot
            .full(pool, class)
            .count(self.area, class.magazine()) as usize;
        let mut done = 0;
        while done < held {
            // A magazine on the stack holds at least one object, so this ends.
            match self.launder_one(pool, class) {
                Some(level) => done += level.max(1),
                None => break,
            }
        }
    }
}
