//! The process generation, and the start-up of the magazine area and the drain.
//!
//! A generation is a random number that names this process for the drain: a drain's claim and
//! its markers carry it, and a claim or marker of any other generation counts as absent. It
//! lives on a page that a forked child reads as zero (`MADV_WIPEONFORK`). The first slow path
//! that finds it zero draws one and installs it with a compare-and-swap; racing drawers agree
//! on the winner's. So a child that inherits a drain cut off by the fork sees its claim and
//! markers as another generation's, and needs no repair pass (`tla/Drain.tla`, protocol
//! "gen").
//!
//! A generation is kept in marker form, the random number's bits above [`MARKER_SHIFT`] with
//! [`MARKER`] below, so the same word is the claim and the marker. A child that drew its parent's number would treat the parent's
//! abandoned claim and markers as live: that slab would stay on the slow path and never be
//! drained, and nothing would be duplicated.
//!
//! The page is the reservation's process page (`Plan::process_page_offset`). The ledger for the
//! unsafe block is `docs/safety/cralloc/depot/generation-safety.md`.

use super::magazine::Area;
use super::{Cache, Depot, Source};
use crate::classes::{MARKER, MARKER_SHIFT, is_marker};
use crate::front::Routines;
use crate::front::sync::AtomicU64;
use crate::os::Os;
use core::num::NonZeroU64;
use core::ptr::{self, NonNull};
use core::sync::atomic::Ordering::{AcqRel, Acquire};

/// The generation page is committed and the area may be carved.
const READY: u64 = 1;
/// The page is wiped on fork and the OS fences restartable sequences: drains may run.
const DRAINS: u64 = 2;

/// A process generation, in marker form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Generation(NonZeroU64);

impl Generation {
    /// The generation for a random number: its high bits, above the marker's low bits. The
    /// high bits are where a multiplicative mix, like the Linux fallback's, puts its entropy.
    pub const fn from_random(random: u64) -> Generation {
        let low = (1u64 << MARKER_SHIFT) - 1;
        // A marker has its lowest bit set, so it is never zero and the fallback never happens.
        match NonZeroU64::new(random & !low | MARKER as u64) {
            Some(word) => Generation(word),
            None => Generation(NonZeroU64::MIN),
        }
    }

    /// The generation a claim or marker word names, if it is one.
    pub fn from_word(word: u64) -> Option<Generation> {
        NonZeroU64::new(word)
            .filter(|w| is_marker(w.get() as usize))
            .map(Generation)
    }

    /// The word a claim holds.
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// The marker a drain of this generation leaves in slab fields.
    pub fn marker(self) -> *mut u8 {
        ptr::without_provenance_mut(self.0.get() as usize)
    }

    /// Whether a slab field holds this generation's marker.
    pub fn is_mine(self, word: *mut u8) -> bool {
        word.addr() as u64 == self.0.get()
    }
}

/// The generation word on the page, which stays mapped for as long as the reservation.
fn word<'a>(page: NonNull<u8>) -> &'a AtomicU64 {
    // SAFETY: GENERATION-01.
    unsafe { &*page.as_ptr().cast::<AtomicU64>() }
}

/// Readies the magazine area and the drain, once per process; later calls return at once.
/// `false` if the generation page cannot be committed, in which case no magazine can be made.
/// Every slow path asks, so the answer once ready is one load, inline, and the work is out of
/// line.
#[inline(always)]
pub(super) fn prepare<O: Os>(depot: &Depot, area: Area<'_, O>) -> bool {
    depot.common.status.load(Acquire) & READY != 0 || prepare_first(depot, area)
}

/// [`prepare`] before the process is ready.
#[cold]
#[inline(never)]
fn prepare_first<O: Os>(depot: &Depot, area: Area<'_, O>) -> bool {
    if depot.common.status.load(Acquire) & READY != 0 {
        return true;
    }
    // Before anything else a first slow path does, and only here: see `domain`. The
    // environment for the same reason, and while it is still the one the process started
    // with (`crate::environment`).
    depot.topology.discover(area.space().os());
    depot.background_choice(area.space().os());
    let page = area.generation_page();
    let space = area.space();
    let len = O::geometry().page_size();
    if space.commit(page, len).is_err() {
        return false;
    }
    // Drains need both: without the wipe a child would take its parent's abandoned claims for
    // live ones, and without the fence a drain cannot know that no sequence is in flight.
    let wiped = space.wipe_on_fork(page, len).is_ok();
    let fenced = wiped && space.os().register_fence().is_ok();
    let status = READY | if fenced { DRAINS } else { 0 };
    depot.common.status.fetch_or(status, AcqRel);
    true
}

/// Whether drains may run.
pub(super) fn drains_enabled(depot: &Depot) -> bool {
    depot.common.status.load(Acquire) & DRAINS != 0
}

/// This process's generation, drawing one if there is none yet. `None` until [`prepare`] has
/// succeeded.
/// With it, whether this call installed it.
pub(super) fn current<O: Os>(depot: &Depot, area: Area<'_, O>) -> (Option<Generation>, bool) {
    if depot.common.status.load(Acquire) & READY == 0 {
        return (None, false);
    }
    let word = word(area.generation_page());
    if let Some(generation) = Generation::from_word(word.load(Acquire)) {
        return (Some(generation), false);
    }
    let drawn = Generation::from_random(area.space().os().random());
    match word.compare_exchange(0, drawn.get(), AcqRel, Acquire) {
        Ok(_) => {
            note_drawn(depot);
            (Some(drawn), true)
        }
        Err(now) => (Generation::from_word(now), false),
    }
}

/// A generation has been drawn in this process or an ancestor (`Common::lineage`).
const DRAWN: u64 = 1;
/// This process is a fork child: it drew a generation where one had been drawn before.
const FORKED: u64 = 2;

/// Records a draw that installed a generation. The lineage word is ordinary memory, which a
/// fork child inherits, while the generation's page is wiped: a process that installs a
/// generation where one was drawn before is a fork child, or a child of one. A child that
/// shares its parent's memory (`vfork`, `CLONE_VM`) shares the generation, draws none, and is
/// not taken for one. A limit: a fork between the install and this record leaves a child that
/// does not know it is one; and where the page cannot be wiped, a child keeps its parent's
/// generation and is not known either (drains are off there).
fn note_drawn(depot: &Depot) {
    let lineage = &depot.common.lineage;
    let bit = if lineage.load(Acquire) & DRAWN != 0 {
        FORKED
    } else {
        DRAWN
    };
    lineage.fetch_or(bit, AcqRel);
}

/// Whether this process is a fork child (see [`note_drawn`]): known once its first slow path
/// has drawn its generation.
pub(super) fn forked(depot: &Depot) -> bool {
    depot.common.lineage.load(Acquire) & FORKED != 0
}

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// Readies the magazine area and the drain if no slow path has yet. `false` if the
    /// magazine area cannot be used, so magazines cannot be had.
    #[inline(always)]
    pub(super) fn prepare(&self) -> bool {
        prepare(self.depot, self.area)
    }

    /// This process's generation, drawing one if there is none yet; `None` before the first
    /// slow path has readied the magazine area.
    ///
    /// The draw in a fork child also makes the background thread's word, which the child
    /// inherited and which may name its parent's thread, the child's: a child begins with no
    /// thread, and its slow paths return memory until it starts one of its own
    /// (`crate::thread_word`, "Whose duty"). And it tells the source, which then distrusts
    /// what a thread cut off by the fork may have left half-changed ([`Source::forked`]).
    pub fn generation(&self) -> Option<Generation> {
        let (generation, drawn) = current(self.depot, self.area);
        if let Some(generation) = generation
            && drawn
            && forked(self.depot)
        {
            self.source
                .thread()
                .make_own(crate::thread_word::key_of(generation.get()));
            self.source.forked();
        }
        generation
    }
}
