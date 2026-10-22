//! Where objects come from when the depot has none, and go when it has too many: the back end.
//!
//! The middle tier never reads or writes an object; it only moves pointers. So the contract
//! below is about which pointers, not about memory, and the trait is safe to implement. The
//! allocator's promise to its callers (a block is free, of its class, and appears once) holds
//! only if the source keeps it.

use crate::classes::SmallClass;
use core::ptr::NonNull;
use core::sync::atomic::AtomicPtr;

/// The back end, as the middle tier sees it.
///
/// Every method may be called from any thread at any time, including from a signal handler
/// that interrupted another call, and must not wait for another thread. Pointers handed out are
/// free objects of the class asked for, each handed out once until it is taken back.
pub trait Source {
    /// Fills the object slots of a magazine this thread holds with free objects of `class`,
    /// up to its capacity, and says how many, `n`. The objects go in the last `n` slots,
    /// lowest address last, so that the magazine, popped from the top, hands them out in
    /// ascending address order. A fill may stop short of the capacity once it has objects, to
    /// bound its work where free objects are sparse. Zero means there are none to be had.
    fn fill(&self, class: SmallClass, slots: &[AtomicPtr<u8>]) -> Filled;

    /// Takes back free objects of `class`, each one this source handed out, from the object
    /// slots of a magazine this thread holds.
    fn take_back(&self, class: SmallClass, objects: &[AtomicPtr<u8>]);

    /// One free object of `class`, for a thread that cannot use a magazine. `None` if there is
    /// none.
    fn alloc_one(&self, class: SmallClass) -> Option<NonNull<u8>>;

    /// Takes back one object of `class`, from a thread that cannot use a magazine.
    fn free_one(&self, class: SmallClass, object: NonNull<u8>);

    /// The count of intervals the heartbeat's clock reads its time from (`crate::clock`), one
    /// for the whole heap, so that the depot's rules and the source's purge tell the same
    /// time.
    fn clock(&self) -> &crate::clock::Counted;

    /// One bounded step of returning free memory to the OS, as the heartbeat paces it. `now`
    /// reads the heartbeat's clock (the counted time of [`Source::clock`]), the same value for
    /// the whole heartbeat; the source calls it only if a decision needs the time.
    fn purge_step(&self, now: &dyn Fn() -> u64);

    /// One bounded step of the clocked pass that is under way or due, for `Cache::maintain`:
    /// whether the pass has more to do, and whether the step returned memory. Called until it
    /// says no more, it does what the purge steps of an interval would, and never goes on into
    /// a later pass.
    fn purge_pass_step(&self, now: &dyn Fn() -> u64) -> PurgeStep;

    /// The background thread's word (`crate::thread_word`).
    fn thread(&self) -> &crate::thread_word::ThreadWord;
}

/// What a fill did ([`Source::fill`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Filled {
    /// How many objects it put in the magazine.
    pub count: usize,
    /// Whether the fill does not continue a run of fills of its class: none has taken objects
    /// of the class from the source for an interval. A hint; the depot hands the objects of
    /// such a fill out in an order that keeps siblings apart (`siblings.rs`).
    pub opens_run: bool,
}

/// What one of `maintain`'s purge steps did ([`Source::purge_pass_step`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PurgeStep {
    /// Another step would do more.
    pub more: bool,
    /// The step returned memory that had been resident: work, which keeps the background
    /// thread from parking (`thread`).
    pub returned: bool,
}
