//! The routines: the only code that writes a slab, apart from the drain.
//!
//! Each routine except [`Routines::cpu`] is one restartable sequence on the slab of the CPU the
//! thread is running on. It reads the CPU number from the thread's rseq area, compares it with
//! the slab count, and leaves without touching anything if it is out of range (as it is for a
//! thread with no registration, which reads a negative number). It commits with its last
//! instruction, a single store to the slab, and before that store it has changed nothing
//! another thread could see. If the OS preempts, migrates or signals the thread, or a fence
//! hits its CPU, before the commit, the sequence starts again from the top, on whatever CPU the
//! thread is then on.
//!
//! In the allocator the routines are assembly generated from Lean. Where assembly cannot run
//! (Kani, Miri, the interleaving checker and the simulated-OS tests) [`super::Twin`] implements
//! the same specifications in Rust. The slow paths are generic over this trait and use nothing
//! else to write a slab.
//!
//! In each specification, `S` is the slab of the CPU the thread reads, `S[f]` the field `f` of
//! it (a byte offset, [`Field::offset`]), and `M[a]` the pointer-sized word at address `a`.
//! "Fail" means the routine returns the stated failure having changed nothing.

use super::slab::{Cpu, Field, Slabs};
use crate::classes::SmallClass;
use core::ptr::NonNull;

/// What [`Routines::store_if_equal`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StoreIfEqual {
    /// The field held the expected value and now holds the new one.
    Stored,
    /// The field held this other value, and still does as far as the routine knows.
    Mismatch(*mut u8),
    /// The thread's CPU number has no slab.
    NoCpu,
}

/// What [`Routines::stack_push`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StackPush {
    /// The magazine is on the stack.
    Pushed,
    /// The head was a marker: the stack is being drained, and the magazine is still the
    /// caller's.
    Marker,
    /// The thread's CPU number has no slab.
    NoCpu,
}

/// What [`Routines::stack_pop`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StackPop {
    /// This magazine was the head; it is the caller's now.
    Popped(NonNull<u8>),
    /// The head was zero or a marker.
    Empty,
    /// The thread's CPU number has no slab.
    NoCpu,
}

/// The routines, and the slab array they work on.
///
/// # Safety
///
/// The slow paths' memory safety and the allocator's correctness rest on each method behaving
/// exactly as specified, as one step that either commits or changes nothing, on the slab of
/// the CPU the thread was running on throughout, and on that being the only CPU whose slab the
/// thread writes. The routines rely in turn on the slab invariant the slow paths maintain:
/// every hot word is zero, a marker, or the address of a slot `s >= 1` of a magazine of its
/// class that nothing else refers to, and every spare head is zero, a marker, or the address
/// of a magazine on that stack.
pub unsafe trait Routines {
    /// The slab array: the static `SLABS` in the allocator.
    fn slabs(&self) -> &Slabs;

    /// The thread's CPU number, if it has a slab: `cpu_id` read from the rseq area and compared
    /// with the slab count. Not a sequence, so the answer may be stale by the time the caller
    /// uses it; the routines that follow check again.
    fn cpu(&self) -> Option<Cpu>;

    /// Pop. With `w = S[hot(class)]` and `mask = class.magazine().mask()`: fail with `None` if
    /// the CPU number is out of range or `w & mask == 0`; otherwise read `p = M[w - 8]`, commit
    /// `S[hot(class)] = w - 8`, and return `p`.
    fn pop(&self, class: SmallClass) -> Option<NonNull<u8>>;

    /// Push. With `w` and `mask` as for pop: fail with `false` if the CPU number is out of
    /// range or `(w + 8) & mask == 0`; otherwise write `M[w] = object`, commit
    /// `S[hot(class)] = w + 8`, and return `true`.
    fn push(&self, class: SmallClass, object: NonNull<u8>) -> bool;

    /// Store-if-equal, the conditional install. With `w = S[field]`: fail with `NoCpu` if the
    /// CPU number is out of range, or with `Mismatch(w)` if `w != expected` (compared as
    /// addresses); otherwise commit `S[field] = new` and return `Stored`.
    ///
    /// # Safety
    ///
    /// `new` keeps the slab invariant for `field`: for a hot word, zero, a marker, or the
    /// address of slot `s`, `1 <= s <= n - 1`, of a magazine of the class's size in committed
    /// magazine memory that the caller holds, with slots `1..s` holding its objects; for a spare
    /// head, zero, a marker, or the slot-0 address of such a magazine whose link keeps the
    /// invariant. The routines that later read the field dereference it.
    unsafe fn store_if_equal(&self, field: Field, expected: *mut u8, new: *mut u8) -> StoreIfEqual;

    /// Stack push. With `h = S[field]`: fail with `NoCpu` if the CPU number is out of range,
    /// or with `Marker` if `h` is a marker ([`crate::classes::is_marker`]); otherwise write
    /// `M[magazine] = h` (the magazine's link, slot 0, which only the caller can see until the
    /// commit), commit `S[field] = magazine`, and return `Pushed`.
    ///
    /// # Safety
    ///
    /// `magazine` is the slot-0 address of a magazine of the size of `field`'s class, in
    /// committed magazine memory, that the caller holds: the routine writes its link, and the
    /// stack pop that later reads the head dereferences it.
    unsafe fn stack_push(&self, field: Field, magazine: NonNull<u8>) -> StackPush;

    /// Stack pop. With `h = S[field]`: fail with `NoCpu` if the CPU number is out of range, or
    /// with `Empty` if `h` is zero or a marker; otherwise read `n = M[h]` (the link, inside the
    /// sequence, which is what rules out ABA), commit `S[field] = n`, and return `Popped(h)`.
    fn stack_pop(&self, field: Field) -> StackPop;
}
