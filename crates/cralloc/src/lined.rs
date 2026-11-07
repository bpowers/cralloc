//! Cache lines for shared state.
//!
//! Two rules keep CPUs from taking lines from each other for nothing, and the types here are
//! how the shared statics follow them:
//!
//! - Data that every operation reads and that is written once, or rarely (a class's layout,
//!   the depot's status), never shares a line with a word that slow paths write.
//! - A word that more than one CPU writes on slow paths shares a line only with words written
//!   by the same event (a depot stack's head with its own water mark), never with another
//!   class's or another purpose's.
//!
//! A struct of shared state puts each group of words in a [`Lined`], and asserts at compile
//! time that its size is the number of lines it means to use (`lines`), so that a field added
//! later without a line of its own fails the build instead of quietly sharing one.
//!
//! Lines are 64 bytes. The measurement behind this is in `docs/research/program-effects.md`,
//! "False sharing": in a two-thread pipeline of large objects, six of every ten cache lines one
//! core took from the other were shared by accident.

use core::ops::Deref;

/// Bytes in a cache line.
pub(crate) const LINE: usize = 64;

/// A value that starts a cache line and shares none with anything else.
#[repr(C, align(64))]
pub(crate) struct Lined<T>(pub(crate) T);

impl<T> Lined<T> {
    pub(crate) const fn new(value: T) -> Lined<T> {
        Lined(value)
    }
}

impl<T> Deref for Lined<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &T {
        &self.0
    }
}

/// The number of whole lines `bytes` takes.
pub(crate) const fn lines(bytes: usize) -> usize {
    bytes.div_ceil(LINE)
}

const _: () = assert!(core::mem::align_of::<Lined<u8>>() == LINE);
const _: () = assert!(core::mem::size_of::<Lined<u64>>() == LINE);
