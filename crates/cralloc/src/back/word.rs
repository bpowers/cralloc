//! Bitmap words: the atomic operations the back end's protocols are made of, and the arithmetic
//! on one word.

use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering::{Release, SeqCst};

/// Bits per bitmap word.
pub(crate) const BITS: u32 = u64::BITS;

/// A shared 64-bit word of back-end state.
///
/// The protocols are written against this trait so the same code runs on the heap's metadata,
/// under Kani on a few words, and under the interleaving explorer in the tests, whose words
/// yield to a scheduler before every operation.
///
/// Every operation but `store` is sequentially consistent. The summary protocol needs that: a
/// free ORs a word and then reads the summary bit above it, while a search clears the summary bit
/// and then reads the word. That is the store-buffering pattern, and only sequential consistency
/// (or fences) rules out both reads missing the other's write, with every operation in the pattern
/// taking part: a failed compare-and-swap is a read too. It also means that an execution of these
/// operations alone is an interleaving, so the TLA+ models and the explorer apply to them as they
/// stand. And an OR's release and a claim's acquire, through whatever else came between them on
/// the word, are all that carries a block's bytes from its last owner to its next
/// (`docs/research/memory-ordering.md`).
///
/// `store` is a release store. It writes only words no protocol races on: a large slot's size
/// word, which only the slot's holder reads or writes, and a slot changes holder only through a
/// sequentially consistent operation on its leaf word (the free's OR, the next claim's
/// compare-and-swap), which orders the old holder's accesses before the new one's; and a region's
/// `Grow` word when `assign` takes or gives back the region, which the table's and the carving
/// word's compare-and-swaps that follow publish to any thread that later reads it.
pub(crate) trait Word: Sync {
    /// Reads the word.
    fn load(&self) -> u64;
    /// Sets `bits` and returns the old value.
    fn fetch_or(&self, bits: u64) -> u64;
    /// Keeps only `bits` and returns the old value.
    fn fetch_and(&self, bits: u64) -> u64;
    /// Replaces `current` with `new`, returning the old value on success and the value found on
    /// failure. A failure changes nothing.
    fn compare_exchange(&self, current: u64, new: u64) -> Result<u64, u64>;
    /// Writes the word, with release ordering. Only for words a single owner writes, such as a
    /// slot's size (see the trait's documentation).
    fn store(&self, value: u64);
}

/// The production word: an `AtomicU64` that offers only the sequentially consistent operations
/// of [`Word`]. Transparent, so a metadata word in the reservation can be viewed as one.
#[repr(transparent)]
pub(crate) struct Atomic(AtomicU64);

impl Atomic {
    /// A word holding zero.
    pub(crate) const fn zero() -> Atomic {
        Atomic(AtomicU64::new(0))
    }
}

impl Word for Atomic {
    #[inline(always)]
    fn load(&self) -> u64 {
        self.0.load(SeqCst)
    }

    #[inline(always)]
    fn fetch_or(&self, bits: u64) -> u64 {
        self.0.fetch_or(bits, SeqCst)
    }

    #[inline(always)]
    fn fetch_and(&self, bits: u64) -> u64 {
        self.0.fetch_and(bits, SeqCst)
    }

    #[inline(always)]
    fn compare_exchange(&self, current: u64, new: u64) -> Result<u64, u64> {
        self.0.compare_exchange(current, new, SeqCst, SeqCst)
    }

    #[inline(always)]
    fn store(&self, value: u64) {
        self.0.store(value, Release)
    }
}

/// The lowest `n` set bits of `word`, or all of them if it has no more than `n`.
#[inline]
pub(crate) const fn lowest(word: u64, n: u32) -> u64 {
    if n >= word.count_ones() {
        return word;
    }
    if n == 0 {
        return 0;
    }
    // Find the largest `keep` whose low `keep` bits hold fewer than `n` set bits, by binary
    // search: six steps whatever `n` is. The `n`-th set bit is then bit `keep`.
    let mut keep = 0;
    let mut step = BITS / 2;
    while step > 0 {
        if (word & low(keep + step)).count_ones() < n {
            keep += step;
        }
        step /= 2;
    }
    word & low(keep + 1)
}

/// The low `bits` bits of a word, `bits <= 64`.
#[inline(always)]
const fn low(bits: u32) -> u64 {
    if bits >= BITS {
        u64::MAX
    } else {
        (1 << bits) - 1
    }
}

/// The bits `first..=last` of a word. `first <= last < 64`.
#[inline(always)]
pub(crate) const fn span(first: u32, last: u32) -> u64 {
    (u64::MAX >> (BITS - 1 - last)) & (u64::MAX << first)
}

/// Which word, and which bit in it, item `index` of a bitmap occupies.
#[inline(always)]
pub(crate) const fn position(index: u32) -> (u32, u32) {
    (index / BITS, index % BITS)
}

/// The bits of word `word` that cover items `first..=last` of a bitmap, or zero if the word
/// covers none of them. `first <= last`.
#[inline]
pub(crate) const fn bits_in_word(word: u32, first: u32, last: u32) -> u64 {
    let base = word as u64 * BITS as u64;
    let (first, last) = (first as u64, last as u64);
    if last < base || first >= base + BITS as u64 {
        return 0;
    }
    let lo = first.saturating_sub(base);
    let hi = if last < base + BITS as u64 - 1 {
        last - base
    } else {
        BITS as u64 - 1
    };
    span(lo as u32, hi as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowest_takes_the_lowest_set_bits() {
        assert_eq!(lowest(0b1011_0110, 2), 0b0000_0110);
        assert_eq!(lowest(0b1011_0110, 3), 0b0001_0110);
        assert_eq!(lowest(0b1011_0110, 9), 0b1011_0110);
        assert_eq!(lowest(u64::MAX, 64), u64::MAX);
        assert_eq!(lowest(u64::MAX, 1), 1);
        assert_eq!(lowest(1 << 63, 1), 1 << 63);
        assert_eq!(lowest(0, 5), 0);
    }

    #[test]
    fn spans_and_word_bits() {
        assert_eq!(span(0, 63), u64::MAX);
        assert_eq!(span(3, 3), 8);
        assert_eq!(span(1, 2), 6);
        assert_eq!(bits_in_word(0, 3, 5), span(3, 5));
        assert_eq!(bits_in_word(1, 3, 5), 0);
        assert_eq!(bits_in_word(1, 60, 70), span(0, 6));
        assert_eq!(bits_in_word(0, 60, 70), span(60, 63));
        assert_eq!(bits_in_word(2, 60, 200), u64::MAX);
        assert_eq!(position(130), (2, 2));
    }
}
