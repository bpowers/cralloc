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
    /// up to its capacity, and returns how many, `n`. The objects go in the last `n` slots,
    /// lowest address last, so that the magazine, popped from the top, hands them out in
    /// ascending address order. A fill may stop short of the capacity once it has objects, to
    /// bound its work where free objects are sparse. Zero means there are none to be had.
    fn fill(&self, class: SmallClass, slots: &[AtomicPtr<u8>]) -> usize;

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

    /// What the purge steps of an interval would do, in one call, for `Cache::maintain`: the
    /// rest of the clocked pass under way or due, and no later one. Its work is bounded by the
    /// source's own bookkeeping, not by the memory it holds.
    fn purge_pass(&self, now: &dyn Fn() -> u64);
}
