//! The heartbeat: the bounded maintenance every slow path does before it returns.
//!
//! At most one magazine laundered, one purge step asked of the source, and one look for an
//! idle cache. There is no background thread and no timer, so maintenance happens at a rate
//! set by allocation activity, in units small enough that no single call pays much.
//!
//! - **Launder.** If the depot stack of the class just served holds more than its target, one
//!   magazine is popped and its objects go back to the source; the empty magazine goes to the
//!   empty stack.
//! - **Purge.** One [`Source::purge_step`].
//! - **Idle check.** Each slow path counts itself in its CPU's activity counter. The check
//!   looks at the next CPU in turn, over every CPU number a slow path has run on, and drains its
//!   slab if the counter has not moved since the previous look and the slab has not been
//!   drained since it last moved. If the slow path met another generation's marker on its own
//!   CPU, it drains that slab instead, since such a slab stays on the slow path until drained.
//!
//! The activity counter and the idle check's record of it are hints in the cold part of the
//! slab, changed with plain atomic instructions rather than routines: a lost update only moves
//! a drain earlier or later.

use super::slow::CHUNK;
use super::{Cache, Source, generation};
use crate::classes::SmallClass;
use crate::front::{Cpu, Routines};
use crate::os::Os;
use core::ptr::NonNull;
use core::sync::atomic::Ordering::Relaxed;

/// What a slow path noticed that its heartbeat should act on.
#[derive(Default, Debug)]
pub(super) struct Chores {
    /// The CPU the slow path ran on, if it had a valid one.
    pub(super) cpu: Option<Cpu>,
    /// It met another generation's marker on that CPU's slab.
    pub(super) stale: bool,
}

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// One heartbeat, after a slow path for `class`.
    pub(super) fn heartbeat(&self, class: SmallClass, chores: Chores) {
        if let Some(cpu) = chores.cpu {
            self.slab(cpu).note_activity();
            if self.depot.cpus.load(Relaxed) <= cpu.index() {
                self.depot.cpus.fetch_max(cpu.index() + 1, Relaxed);
            }
        }
        self.launder(class);
        self.source.purge_step();
        if !generation::drains_enabled(self.depot) {
            return;
        }
        match chores.cpu {
            Some(cpu) if chores.stale => {
                self.drain(cpu);
            }
            _ => self.idle_check(),
        }
    }

    /// Launders one magazine of `class` if its depot stack is over its target.
    fn launder(&self, class: SmallClass) {
        let target = self.tuning.depot_target / class.size();
        let stack = self.depot.full(class);
        if (stack.objects(self.area, class.magazine()) as usize) <= target {
            return;
        }
        let Some(magazine) = stack.pop(self.area, class.magazine()) else {
            return;
        };
        let level = magazine.level();
        let mut buffer = [NonNull::dangling(); CHUNK];
        let mut done = 0;
        while done < level {
            let count = (level - done).min(CHUNK);
            for (i, slot) in buffer[..count].iter_mut().enumerate() {
                match NonNull::new(magazine.object(done + i + 1)) {
                    Some(object) => *slot = object,
                    None => self.os().abort("a magazine holds a null object"),
                }
            }
            self.source.take_back(class, &buffer[..count]);
            done += count;
        }
        magazine.set_level(0);
        self.to_depot(class, magazine);
    }

    /// Looks at the next CPU and drains its slab if it has been idle since the last look.
    fn idle_check(&self) {
        let count = self.depot.cpus.load(Relaxed);
        if count == 0 {
            return;
        }
        let next = self.depot.cursor.fetch_add(1, Relaxed) % count;
        let Some(cpu) = self.routines.slabs().cpu(next as i32) else {
            return;
        };
        let slab = self.slab(cpu);
        let activity = slab.activity();
        if activity == 0 {
            return;
        }
        // The record is the activity seen at the last look, shifted up, with the low bit set
        // once the slab has been drained at that activity.
        let quiet = activity << 1;
        let seen = slab.seen().load(Relaxed);
        if seen == quiet | 1 {
            return;
        }
        if seen != quiet {
            slab.seen().store(quiet, Relaxed);
            return;
        }
        if self.drain(cpu) == super::Drained::Done {
            slab.seen().store(quiet | 1, Relaxed);
        }
    }
}
