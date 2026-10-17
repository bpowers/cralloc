//! Where objects come from when the depot has none, and go when it has too many: the back end.
//!
//! The middle tier never reads or writes an object; it only moves pointers. So the contract
//! below is about which pointers, not about memory, and the trait is safe to implement. The
//! allocator's promise to its callers (a block is free, of its class, and appears once) holds
//! only if the source keeps it.

use crate::classes::SmallClass;
use core::ptr::NonNull;

/// The back end, as the middle tier sees it.
///
/// Every method may be called from any thread at any time, including from a signal handler
/// that interrupted another call, and must not wait for another thread. Pointers handed out are
/// free objects of the class asked for, each handed out once until it is taken back.
pub trait Source {
    /// Fills `out` from the front with free objects of `class`, up to its length, and returns
    /// how many. Zero means there are none to be had.
    fn fill(&self, class: SmallClass, out: &mut [Option<NonNull<u8>>]) -> usize;

    /// Takes back free objects of `class`, each one this source handed out.
    fn take_back(&self, class: SmallClass, objects: &[NonNull<u8>]);

    /// One free object of `class`, for a thread that cannot use a magazine. `None` if there is
    /// none.
    fn alloc_one(&self, class: SmallClass) -> Option<NonNull<u8>>;

    /// Takes back one object of `class`, from a thread that cannot use a magazine.
    fn free_one(&self, class: SmallClass, object: NonNull<u8>);

    /// One bounded step of returning free memory to the OS, as the heartbeat paces it.
    fn purge_step(&self);
}
