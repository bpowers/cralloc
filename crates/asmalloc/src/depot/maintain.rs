//! `maintain`: the middle tier's share of one round of the clocked maintenance for the whole
//! heap, for a background thread or a program's idle hook. It does at once what the heartbeats
//! of an interval would have done, and respects the clock as they do, so a second call within
//! an interval finds nothing to do.
//!
//! 1. **The idle look at every slab** a slow path has used ([`Cache::idle_look`]): a slab whose
//!    activity counter has stood still for an interval is drained, including one busy only on
//!    its fast paths, which is the case no heartbeat can see from its own CPU. Its CPU's next
//!    calls then take slow paths, whose heartbeats run that CPU's own age and sweep rules. The
//!    drain launders what it takes rather than parking it in the depot (`drain.rs`, "what it
//!    takes"): that is what the age limit would have done there, and a slab busy on its fast
//!    paths would otherwise pop its own magazines straight back. A slab is drained at most
//!    once per interval: the sighting a drain leaves waits for the counter to move, and then
//!    for an interval of quiet after that.
//! 2. **Every depot stack's shedding** (`shed.rs`): a look at the depth of every class's stack,
//!    which starts a new interval for each that is due and sets what it owes, and then the
//!    whole of every debt laundered.
//! 3. **The clocked purge** ([`Source::purge_pass`]): the pass that is under way or due, run
//!    to its end.
//!
//! The thread that calls it counts no activity on its own CPU and runs no age look there:
//! `maintain` is not a slow path, and if it counted, the slab of the CPU it happens to run on
//! would never look idle. The clock is read once.
//!
//! **Work.** Bounded by the heap's bookkeeping, not its memory: a look at each slab in use (a
//! load or two, and a compare-and-swap when its counter moved), at most one drain each (a
//! bounded number of atomic instructions, one fence, and a launder of each magazine taken), a
//! look at each class's stack, launders of what the stacks owe (at most the magazines on
//! them), and one lap of the bitmaps at most for the purge. With nothing to do it is one clock
//! read, two loads per slab in use, a few per class, and the purge's look at its decay word.
//!
//! **Fork.** Every step is a step some slow path or `collect` already takes, with the same
//! single-instruction commits, so a fork in the middle of `maintain` leaves the child what a
//! fork in the middle of a slow path leaves it (design: "Process lifecycle", "Fork"). The child
//! does not inherit the thread that called it.

use super::drain::Disposal;
use super::heartbeat::Clock;
use super::{Cache, Source, generation};
use crate::front::Routines;
use crate::os::Os;
use core::sync::atomic::Ordering::Acquire;

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// One round of the clocked maintenance for the whole heap (see the module documentation).
    pub fn maintain(&self) {
        let clock = Clock::new(self.os());
        if generation::drains_enabled(self.depot) {
            let count = self.depot.shared.cpus.load(Acquire);
            for index in 0..count {
                if let Some(cpu) = self.routines.slabs().cpu(index as i32) {
                    self.idle_look(cpu, clock.now(), Disposal::Launder);
                }
            }
        }
        if self.tuning.shed_unused {
            for class in crate::classes::SmallClass::all() {
                self.look_at_depth(class, clock.now());
            }
            self.shed_all();
        }
        self.source.purge_pass(&|| clock.now());
    }
}
