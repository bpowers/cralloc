//! Which domains' stacks hold magazines, so that the heartbeat can find a quiet domain's stacks
//! without reading every domain's.
//!
//! A domain whose CPUs have gone quiet runs no heartbeat of its own, so its stacks shed only if
//! another domain's heartbeats look at them (`shed.rs`). A look at one class in every domain
//! would read a stack head in each of up to 64 domains, whatever the process uses. Instead each
//! domain keeps the set of classes whose stacks may hold magazines, and the depot the set of
//! domains whose set is not empty, and each periodic look visits one other domain, chosen from
//! those sets in rotation: the `t`-th look of a CPU visits, of the `n` other domains that hold
//! something, the `(t mod n)`-th, and looks at every stack its set names, unless one of that
//! domain's own CPUs has looked at its stacks in this interval (each domain records the last
//! interval it did, one store an interval). A domain in use sheds through its own looks, and a
//! look from another L3 at each of its stacks read, and sometimes wrote, its busiest lines:
//! that cost three independent pipelines spread over two L3s 6%. So a look's work
//! grows with the classes one domain holds, never with the number of domains, and while the
//! sets stay as they are a CPU's looks visit every held stack of the other domains once in
//! every `n` looks. A stack owes nothing until a second look, in a later interval than the
//! first, sees it still deep, so a quiet domain's stacks go back within two rounds of looks
//! and an interval: with one CPU at 640 slow paths an interval, a look in every
//! [`LOOK_EVERY`](super::LOOK_EVERY), and 63 quiet domains, four intervals. Visiting one stack
//! a look, as this did first, took sixteen with five classes a domain.
//!
//! **Hints.** The sets change by single atomic instructions and only steer looks: a bit that is
//! set on a stack that is empty costs a look that finds nothing, and the look clears it. A bit
//! must not stay clear on a stack that holds a magazine, or a quiet domain would keep it until
//! `collect`, which looks at everything (`maintain` looks by the hints too). So:
//!
//! - a push onto an empty stack (`Stack::push_under` says so), after its compare-and-swap, sets
//!   the class's bit if it reads it clear, and then the domain's bit if it reads that clear;
//! - a look that finds a stack empty clears the class's bit, reads the head again, and sets the
//!   bit back if a push came; whoever sets a class bit back then makes sure of the domain's; and
//!   a look that leaves a domain's set empty clears the domain's bit, reads the set again, and
//!   sets the bit back if a class came.
//!
//! Every one of these operations, the push's compare-and-swap included, is sequentially
//! consistent, so of a push and a clear that race, either the clear's second read sees the push
//! or the push's read sees the clear; either way the bit ends set. On x86-64 that costs nothing:
//! every read-modify-write is a full barrier there and every load is already ordered.
//!
//! A fork can cut a push off between its compare-and-swap and its bits, leaving the child a stack
//! with no bit; the child's `collect` and that domain's own heartbeats, which look at each class
//! in turn whatever the hints say, still reach it.
//!
//! Only domain pools keep the sets. The shared stacks are looked at by every look.

use super::domain::Domain;
use super::shed::interval;
use super::{Cache, Pool, Source};
use crate::classes::{SMALL_COUNT, SmallClass};
use crate::front::Routines;
use crate::os::Os;
use core::sync::atomic::Ordering::{Relaxed, SeqCst};

/// The position of the `n`-th set bit of `mask`, counting from the lowest, for `n` below its
/// count of set bits.
fn nth_set_bit(mut mask: u64, n: u32) -> u32 {
    for _ in 0..n {
        mask &= mask.wrapping_sub(1);
    }
    mask.trailing_zeros()
}

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// Notes that `class`'s stack in `domain` holds a magazine, after a push onto it found it
    /// empty.
    pub(super) fn hold(&self, domain: Domain, class: SmallClass) {
        let bit = 1u64 << (class.ordinal() % SMALL_COUNT);
        let held = self.depot.held(domain);
        if held.load(SeqCst) & bit == 0 {
            held.fetch_or(bit, SeqCst);
        }
        self.hold_domain(domain);
    }

    /// Makes sure `domain`'s bit is set, after a class bit of it was seen or made set.
    fn hold_domain(&self, domain: Domain) {
        let bit = 1u64 << domain.index();
        if self.depot.held_domains.load(SeqCst) & bit == 0 {
            self.depot.held_domains.fetch_or(bit, SeqCst);
        }
    }

    /// Clears the bit of `class`'s stack in `domain` if the stack is empty, unless a push came
    /// meanwhile; then `domain`'s own bit if none of its classes is left.
    pub(super) fn unhold(&self, domain: Domain, class: SmallClass) {
        let bit = 1u64 << (class.ordinal() % SMALL_COUNT);
        let held = self.depot.held(domain);
        let stack = self.depot.full(Pool::Domain(domain), class);
        if held.load(Relaxed) & bit == 0 || stack.holds_any() {
            return;
        }
        held.fetch_and(!bit, SeqCst);
        if stack.holds_any() {
            held.fetch_or(bit, SeqCst);
            self.hold_domain(domain);
            return;
        }
        self.unhold_domain(domain);
    }

    /// Clears `domain`'s bit if none of its classes' bits is set, unless one is set meanwhile.
    fn unhold_domain(&self, domain: Domain) {
        let held = self.depot.held(domain);
        if held.load(SeqCst) != 0 {
            return;
        }
        let bit = 1u64 << domain.index();
        self.depot.held_domains.fetch_and(!bit, SeqCst);
        if held.load(SeqCst) != 0 {
            self.depot.held_domains.fetch_or(bit, SeqCst);
        }
    }

    /// Records that a CPU of `here` looks at its own stacks in the interval of `now`, unless one
    /// already has: one store a domain an interval.
    pub(super) fn note_looked(&self, here: Domain, now: u64) {
        if self.depot.domain_count() < 2 {
            return;
        }
        let stamp = interval(now) + 1;
        let looked = self.depot.looked(here);
        if looked.load(Relaxed) != stamp {
            looked.store(stamp, Relaxed);
        }
    }

    /// The `turn`-th look's visit to a domain other than `here`: every held class of one held
    /// domain, the domains in rotation (see the module documentation), looked at with the
    /// clock at `now`, unless that domain's own CPUs have looked at its stacks in this
    /// interval.
    pub(super) fn look_elsewhere(&self, here: Domain, turn: u64, now: u64) {
        if self.depot.domain_count() < 2 {
            return;
        }
        let others = self.depot.held_domains.load(Relaxed) & !(1u64 << here.index());
        if others == 0 {
            return;
        }
        let n = u64::from(others.count_ones());
        let domain = Domain::folded(nth_set_bit(others, (turn % n) as u32) as usize);
        if self.depot.looked(domain).load(Relaxed) == interval(now) + 1 {
            return;
        }
        let classes = self.depot.held(domain).load(Relaxed);
        if classes == 0 {
            self.unhold_domain(domain);
            return;
        }
        let mut rest = classes;
        while rest != 0 {
            let ordinal = rest.trailing_zeros() as usize;
            rest &= rest - 1;
            if let Some(class) = SmallClass::from_ordinal(ordinal) {
                self.look_at_depth(Pool::Domain(domain), class, now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nth_set_bit_counts_from_the_lowest() {
        let mask = 0b1011_0100u64;
        let bits: std::vec::Vec<u32> = (0..mask.count_ones())
            .map(|n| nth_set_bit(mask, n))
            .collect();
        assert_eq!(bits, [2, 4, 5, 7]);
        assert_eq!(nth_set_bit(1 << 63, 0), 63);
        assert_eq!(nth_set_bit(u64::MAX, 63), 63);
    }
}
