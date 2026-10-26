//! The back end as the depot's [`Source`].
//!
//! The depot hands over a magazine's object slots, which the back end fills and launders in
//! place (`back::Descending`, `back::SlotPointer`).

use crate::back::policy::FILL_CLAIMS;
use crate::back::{Back, Descending, PassStep};
use crate::classes::SmallClass;
use crate::depot::{Filled, PurgeStep, Source};
use crate::os::Os;
use core::ptr::NonNull;
use core::sync::atomic::AtomicPtr;

impl<O: Os> Source for Back<'_, O> {
    fn clock(&self) -> &crate::clock::Counted {
        Back::clock(self)
    }

    fn fill(&self, class: SmallClass, slots: &[AtomicPtr<u8>]) -> Filled {
        let filled = Back::fill_within(self, class, &mut Descending(slots), FILL_CLAIMS);
        Filled {
            count: filled.count,
            opens_run: filled.opens_run,
        }
    }

    fn take_back(&self, class: SmallClass, objects: &[AtomicPtr<u8>]) {
        // SAFETY: HEAP-SOURCE-01.
        unsafe { self.launder(class, objects) };
    }

    fn alloc_one(&self, class: SmallClass) -> Option<NonNull<u8>> {
        self.alloc(class.class(), class.size())
            .map(|block| block.ptr)
    }

    fn free_one(&self, class: SmallClass, object: NonNull<u8>) {
        self.free(object, class.class());
    }

    fn purge_step(&self, now: &dyn Fn() -> u64, seen: crate::thread_word::Snapshot) -> bool {
        self.heartbeat_for(now, seen)
    }

    fn thread_step(&self, now: &dyn Fn() -> u64) -> PurgeStep {
        let step = self.thread_purge_steps(now);
        PurgeStep {
            more: step.more,
            returned: step.returned != 0,
        }
    }

    fn plan_thread(&self) {
        self.plan_hold();
    }

    fn thread_due(&self, now: &dyn Fn() -> u64) -> bool {
        self.purge_due(now)
    }

    fn purge_pass_step(&self, now: &dyn Fn() -> u64) -> PurgeStep {
        // The hold first: what it returns down to its floor is then not in the count the
        // clocked pass takes its goal from.
        let hold = self.hold_maintain_step(now);
        let pass = if hold.more {
            PassStep::default()
        } else {
            self.decay_pass_step(now)
        };
        PurgeStep {
            more: hold.more || pass.more,
            returned: hold.returned != 0 || pass.returned != 0,
        }
    }

    fn thread(&self) -> &crate::thread_word::ThreadWord {
        Back::thread(self)
    }

    fn forked(&self) {
        self.distrust_tails();
    }
}
