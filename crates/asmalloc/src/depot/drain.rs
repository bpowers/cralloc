//! The drain of an idle slab (`tla/Drain.tla`, protocol "gen").
//!
//! 1. Claim the slab by writing this generation into its claim with a compare-and-swap. If it
//!    already holds this generation, another thread is draining: stop. A claim of any other
//!    generation was left by a drain a fork cut off, and counts as absent.
//! 2. Exchange each class's hot word and spare heads with this generation's marker, one atomic
//!    instruction each, keeping the old values.
//! 3. Fence the slab's CPU, so that every sequence in flight there has completed or been
//!    aborted. If the fence fails, put each old value back with a compare-and-swap against the
//!    marker (a marker comes back as zero), release the claim, and stop.
//! 4. Read each field again. If it still holds the marker, nothing can commit to it any more and
//!    the drainer owns the old value: it clears the marker (with a compare-and-swap, see below)
//!    and sends whatever magazines the old value names to the depot. If it holds anything else, a sequence that started before step 2
//!    committed, the old value still belongs to that CPU, and the drainer takes nothing and
//!    writes nothing.
//! 5. Release the claim with a compare-and-swap from this generation, since a forked child may
//!    have taken over a claim this drain left.
//!
//! The routines treat any marker as no magazine or an empty stack and never store over one, and
//! the slow paths never install over this generation's marker, so only a commit from a sequence
//! in flight at step 2 can replace a marker, and step 3 ends those. The claim means a slab has
//! one drainer at a time, so a marker a drainer finds in step 4 is its own. A marker of another
//! generation that step 2 displaces is nothing to take.
//!
//! The drain is the only code that writes one CPU's slab from another, and it never waits: each
//! step is a bounded number of atomic instructions or one system call.

use super::magazine::Magazine;
use super::{Cache, Source, generation};
use crate::classes::{SMALL_COUNT, SmallClass, is_marker};
use crate::front::{Cpu, Field, Routines, Spare};
use crate::os::Os;
use core::ptr;
use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed};

/// How a drain ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drained {
    /// Drains are off: the OS cannot fence or cannot wipe a page on fork, or the magazine area
    /// is not ready.
    Disabled,
    /// Another thread of this generation holds the claim, or took it first.
    Busy,
    /// The fence failed; every field this drain marked is as it was or has moved on.
    BackedOut,
    /// The slab was drained.
    Done,
}

/// The fields of a class the drain exchanges, in the order it keeps their old values.
const KINDS: [Option<Spare>; 3] = [None, Some(Spare::Full), Some(Spare::Empty)];

fn field(class: SmallClass, kind: Option<Spare>) -> Field {
    match kind {
        None => Field::hot(class),
        Some(spare) => Field::spare(class, spare),
    }
}

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// Drains the slab of `cpu`, sending everything cached there to the depot.
    pub fn drain(&self, cpu: Cpu) -> Drained {
        if !generation::drains_enabled(self.depot) {
            return Drained::Disabled;
        }
        let Some(generation) = self.generation() else {
            return Drained::Disabled;
        };
        let slab = self.slab(cpu);
        let marker = generation.marker();

        // Step 1.
        let claim = slab.claim().load(Acquire);
        if claim == generation.get()
            || slab
                .claim()
                .compare_exchange(claim, generation.get(), AcqRel, Acquire)
                .is_err()
        {
            return Drained::Busy;
        }

        // Step 2.
        let mut old = [[ptr::null_mut::<u8>(); 3]; SMALL_COUNT];
        for class in SmallClass::all() {
            for (k, kind) in KINDS.into_iter().enumerate() {
                old[class.ordinal() % SMALL_COUNT][k] =
                    slab.field(field(class, kind)).swap(marker, AcqRel);
            }
        }

        // Step 3.
        if self.os().fence(cpu.get()).is_err() {
            for class in SmallClass::all() {
                for (k, kind) in KINDS.into_iter().enumerate() {
                    let was = old[class.ordinal() % SMALL_COUNT][k];
                    let restore = if is_marker(was.addr()) {
                        ptr::null_mut()
                    } else {
                        was
                    };
                    // A failure means a sequence in flight committed over the marker; the old
                    // value is then the CPU's, as in step 4.
                    let _ = slab
                        .field(field(class, kind))
                        .compare_exchange(marker, restore, AcqRel, Relaxed);
                }
            }
            self.release(cpu, generation.get());
            return Drained::BackedOut;
        }

        // Step 4.
        for class in SmallClass::all() {
            for (k, kind) in KINDS.into_iter().enumerate() {
                let current = slab.field(field(class, kind));
                if !generation.is_mine(current.load(Acquire)) {
                    continue;
                }
                // The marker was still there after the fence, so the old value is this
                // drain's. Clearing it takes a compare-and-swap: in the parent nothing can
                // replace the marker now, but in a child forked from a signal handler during
                // this drain the marker is another generation's, which a slow path may already
                // have installed over, and that magazine must not be overwritten.
                let _ = current.compare_exchange(marker, ptr::null_mut(), AcqRel, Relaxed);
                let was = old[class.ordinal() % SMALL_COUNT][k];
                if was.is_null() || is_marker(was.addr()) {
                    continue;
                }
                match kind {
                    None => self.take_loaded(class, was.addr()),
                    Some(_) => self.take_chain(class, was.addr()),
                }
            }
        }

        // Step 5.
        self.release(cpu, generation.get());
        Drained::Done
    }

    fn release(&self, cpu: Cpu, generation: u64) {
        let _ = self
            .slab(cpu)
            .claim()
            .compare_exchange(generation, 0, AcqRel, Relaxed);
    }

    /// Sends the magazine a hot word named to the depot, at the level the word gave it.
    fn take_loaded(&self, class: SmallClass, word: usize) {
        match self.area.loaded(word, class.magazine()) {
            Some((magazine, level)) => {
                magazine.set_level(level);
                self.to_depot(class, magazine);
            }
            None => self
                .os()
                .abort("a slab word names something that is not a magazine"),
        }
    }

    /// Sends every magazine of a spare stack to the depot, each at its recorded level.
    fn take_chain(&self, class: SmallClass, head: usize) {
        let mut next = head;
        while next != 0 {
            let magazine: Magazine<'_> = match self.area.magazine(next, class.magazine()) {
                Some(magazine) => magazine,
                None => self
                    .os()
                    .abort("a spare stack names something that is not a magazine"),
            };
            // The link must be read before the depot overwrites it.
            next = magazine.link().load(Acquire).addr();
            magazine.set_level(magazine.level());
            self.to_depot(class, magazine);
        }
    }
}
