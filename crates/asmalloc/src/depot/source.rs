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

    /// The heartbeat's part in returning free memory to the OS. `now` reads the heartbeat's
    /// clock (the counted time of [`Source::clock`]), the same value for the whole heartbeat;
    /// the source calls it only if a decision needs the time. `seen` is the background
    /// thread's word as the slow path read it. Where slow paths return memory, one bounded step
    /// of it. While the thread runs, none, and says whether work the thread does has just come
    /// due, for a thread that sleeps, which the caller then wakes (`thread`).
    fn purge_step(&self, now: &dyn Fn() -> u64, seen: crate::thread_word::Snapshot) -> bool;

    /// One bounded step of each purge that slow paths take where there is no background
    /// thread, for the thread, which takes them while it runs: whether another would do more
    /// now, and whether this one returned memory.
    fn thread_step(&self, now: &dyn Fn() -> u64) -> PurgeStep;

    /// After the thread's steps: plans when it next has to look, where it can foresee that, for
    /// the slow path that reaches the moment to wake it.
    fn plan_thread(&self);

    /// Whether purge work is due that the thread should do before it sleeps: its last look,
    /// after it marked its word asleep (`crate::thread_word`, "Sleeping").
    fn thread_due(&self, now: &dyn Fn() -> u64) -> bool;

    /// One bounded step of the clocked pass that is under way or due, for `Cache::maintain`:
    /// whether the pass has more to do, and whether the step returned memory. Called until it
    /// says no more, it does what the purge steps of an interval would, and never goes on into
    /// a later pass.
    fn purge_pass_step(&self, now: &dyn Fn() -> u64) -> PurgeStep;

    /// The background thread's word (`crate::thread_word`).
    fn thread(&self) -> &crate::thread_word::ThreadWord;

    /// In the child of a fork, at its first slow path: state that a thread the fork cut off
    /// may have left half-changed is to be distrusted (the back end's huge-page tails,
    /// `back::tail`, "Repair").
    fn forked(&self) {}
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
