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
//!
//! **Memory order.** A routine's commit is a plain store, so step 2's exchange can read a commit
//! without acquiring anything from it, and the commit can be visible before the slots it covers
//! are: nothing here reads a slot or a link until the fence has returned. The fence is what carries
//! everything the CPU's threads wrote to the drainer, the objects' bytes among it: it is a barrier
//! on that CPU, or the scheduler's when no thread of the process runs there, between the drainer's
//! barrier before it and its barrier after (`Os::fence`). So step 4's read sees any commit that
//! beat the fence, and a sequence that starts after it sees the marker, whatever orders the loads
//! and stores have. What the drainer then takes goes on by the depot's release, or a launder's.
//!
//! What it takes goes to the depot, where the heartbeat's idle check and `collect` want it: a
//! slow path may launder only a few magazines, and `collect` launders the depot next. The
//! background thread's `maintain` launders what it takes instead ([`Disposal::Launder`]),
//! which is what the age limit would have done on that CPU's own slow paths: a slab busy only
//! on its fast paths would otherwise pop its drained magazines straight back from the depot,
//! and the objects in them, scattered wherever the heap once was, would pin their pages for
//! ever.

use super::domain::Domain;
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

/// What a drain does with the magazines it takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Disposal {
    /// Sends them to the depot as they are.
    Park,
    /// Returns their objects to the source and sends them to the depot empty.
    Launder,
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
        self.drain_with(cpu, Disposal::Park)
    }

    /// Drains the slab of `cpu`, disposing of what it takes as `disposal` says.
    pub(super) fn drain_with(&self, cpu: Cpu, disposal: Disposal) -> Drained {
        if !generation::drains_enabled(self.depot) {
            return Drained::Disabled;
        }
        let Some(generation) = self.generation() else {
            return Drained::Disabled;
        };
        let slab = self.slab(cpu);
        let marker = generation.marker();
        // What the drain takes stays in the drained CPU's cache domain.
        let domain = self.depot.domain_of(Some(cpu));

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

        // Step 4, in two passes: every field is settled and its marker cleared first, so that
        // the CPU's fast paths have the whole slab back before the slow work of disposing of
        // what was taken (a launder, for `Disposal::Launder`). A field still marked would send
        // every call of its class there to a slow path meanwhile.
        for class in SmallClass::all() {
            for (k, kind) in KINDS.into_iter().enumerate() {
                let current = slab.field(field(class, kind));
                let was = &mut old[class.ordinal() % SMALL_COUNT][k];
                if !generation.is_mine(current.load(Acquire)) {
                    // A sequence in flight committed: the old value is still the CPU's.
                    *was = ptr::null_mut();
                    continue;
                }
                // The marker was still there after the fence, so the old value is this
                // drain's. Clearing it takes a compare-and-swap: in the parent nothing can
                // replace the marker now, but in a child forked from a signal handler during
                // this drain the marker is another generation's, which a slow path may already
                // have installed over, and that magazine must not be overwritten.
                let _ = current.compare_exchange(marker, ptr::null_mut(), AcqRel, Relaxed);
            }
        }
        for class in SmallClass::all() {
            for (k, kind) in KINDS.into_iter().enumerate() {
                let was = old[class.ordinal() % SMALL_COUNT][k];
                if was.is_null() || is_marker(was.addr()) {
                    continue;
                }
                match kind {
                    None => self.take_loaded(domain, class, was.addr(), disposal),
                    Some(_) => self.take_chain(domain, class, was.addr(), disposal),
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

    /// Disposes of a magazine the drain took from a slab of `domain`, to that domain's depot.
    fn dispose(
        &self,
        domain: Domain,
        class: SmallClass,
        magazine: Magazine<'_>,
        disposal: Disposal,
    ) {
        if disposal == Disposal::Launder {
            self.launder_held(class, &magazine);
        }
        self.to_domain(domain, class, magazine);
    }

    /// Disposes of the magazine a hot word named, at the level the word gave it.
    fn take_loaded(&self, domain: Domain, class: SmallClass, word: usize, disposal: Disposal) {
        match self.area.loaded(word, class.magazine()) {
            Some((magazine, level)) => {
                magazine.set_level(level);
                self.dispose(domain, class, magazine, disposal);
            }
            None => self
                .os()
                .abort("a slab word names something that is not a magazine"),
        }
    }

    /// Disposes of every magazine of a spare stack, each at its recorded level.
    fn take_chain(&self, domain: Domain, class: SmallClass, head: usize, disposal: Disposal) {
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
            self.dispose(domain, class, magazine, disposal);
        }
    }
}
