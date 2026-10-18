//! The depot's low-water rule: a stack sheds the depth it did not need.
//!
//! Per class the depot keeps a [`Water`] word: the lowest depth the class's stack reached during
//! the current interval, and what the interval before left it owing. At the first look in a new
//! interval, the old interval's low-water mark is how much of the stack nobody used, and it
//! becomes what the stack owes. The heartbeats that follow launder it back to the source, one
//! magazine each, from whichever stack owes, whatever class their slow path served. A stack that
//! nothing touched for an interval is the case where the mark is its whole depth. A stack in use
//! keeps what it needed: after it sheds, its consumers take its old bottom and fill the rest from
//! the source, which gives them the lowest free addresses.
//!
//! **Units.** Depth counts objects, not magazines. Objects are what pin pages, and the stack
//! already keeps an exact count of them in its top magazine's link, so a look costs two loads
//! and adds nothing to pushes and pops; a count of magazines would need a new field that every
//! push and pop writes. A stack of many nearly empty magazines, such as drains leave, looks
//! small in objects, but that does not hide them: a shed pops whole magazines until it has
//! laundered what is owed, so such a stack sheds many of them, and their empty shells go to the
//! empty stack for reuse. A shed may overshoot by part of one magazine, which costs a refill
//! sooner.
//!
//! **Where depth is seen.** Only pops lower a stack, and every pop of a class's stack is followed
//! by a note of its depth, except `collect`'s, which empties the stack: that only lets the next
//! debt overstate a stack that is then empty, which is harmless. The slow path that took a
//! magazine notes the depth in its heartbeat, and the launders note it as they pop. A note reads
//! no clock, except the first for a class, which starts its word; so a note counts toward the
//! interval the word is in. Intervals begin at looks: every [`Tuning::look_every`](super::Tuning)
//! heartbeats a CPU looks at one class's stack, the classes in turn, with the clock it reads for
//! the idle check, and a look in a new interval starts it and sets the debt. A note or look reads
//! the stack's head and its top magazine's link, and writes the water word only when the depth
//! is a new low, the interval has changed, or a shed paid something, so a pop costs no shared
//! write beyond its own in the common case.
//!
//! **Hints.** The water word and the set of classes that owe ([`Depot`](super::Depot)'s
//! `owing`) change by single atomic instructions and never touch the stack head, so they add
//! nothing to the stack's ABA argument: a shed pops with the stack's own pop. A race between
//! looks, a clock that differs between CPUs or goes back, or a depth read while it changes makes
//! a stack shed a magazine early or late, more or less, and never anything worse: shedding is a
//! launder, which is safe at any time.

use super::domain::Domain;
use super::heartbeat::{Clock, IDLE_SHIFT};
use super::{Cache, Source};
use crate::classes::{SMALL_COUNT, SmallClass};
use crate::front::Routines;
use crate::os::Os;
use core::sync::atomic::Ordering::{Relaxed, SeqCst};

/// Bits of a depth or a debt in objects; more saturates.
const COUNT_BITS: u32 = 24;
const COUNT_MAX: u32 = (1 << COUNT_BITS) - 1;
const OWED_SHIFT: u32 = COUNT_BITS;
const INTERVAL_SHIFT: u32 = 2 * COUNT_BITS;
/// Bits of the interval number a word keeps.
const INTERVAL_BITS: u32 = 15;
const INTERVAL_MASK: u64 = (1 << INTERVAL_BITS) - 1;
/// Set once a look has written the word: all zero is a stack never looked at.
const SEEN: u64 = 1 << 63;

const _: () = assert!(INTERVAL_SHIFT + INTERVAL_BITS == 63);
const _: () = assert!(SMALL_COUNT <= 64);

/// The interval `now` falls in, as a water word keeps it.
pub(super) fn interval(now: u64) -> u64 {
    (now >> IDLE_SHIFT) & INTERVAL_MASK
}

/// One class's low-water word: the lowest depth of its stack seen this interval, in objects
/// (bits 0 to 23), what the last interval left it owing, in objects (bits 24 to 47), which
/// interval this is (bits 48 to 62, truncated), and whether any look has written it (bit 63).
///
/// The interval number is truncated to 15 bits: a gap of 2^14 intervals or more (about four and
/// a half hours) between looks reads as a clock that went back, which re-bases the word and
/// postpones the stack's shed by one interval.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Water(pub(super) u64);

impl Water {
    fn pack(low: u32, owed: u32, interval: u64) -> Water {
        Water(
            SEEN | (interval & INTERVAL_MASK) << INTERVAL_SHIFT
                | (owed.min(COUNT_MAX) as u64) << OWED_SHIFT
                | low.min(COUNT_MAX) as u64,
        )
    }

    /// Whether a look has started the word.
    pub(super) fn seen(self) -> bool {
        self.0 & SEEN != 0
    }

    /// The lowest depth seen this interval.
    pub(super) fn low(self) -> u32 {
        (self.0 as u32) & COUNT_MAX
    }

    /// The objects the stack owes the source.
    pub(super) fn owed(self) -> u32 {
        ((self.0 >> OWED_SHIFT) as u32) & COUNT_MAX
    }

    fn interval(self) -> u64 {
        self.0 >> INTERVAL_SHIFT & INTERVAL_MASK
    }

    /// What a look that finds the stack `depth` objects deep in interval `interval` makes of
    /// this word: a new low in the same interval, or a new interval, which owes the old low.
    pub(super) fn look(self, depth: u32, interval: u64) -> Water {
        let depth = depth.min(COUNT_MAX);
        if self.0 & SEEN == 0 {
            return Water::pack(depth, 0, interval);
        }
        let ahead = interval.wrapping_sub(self.interval()) & INTERVAL_MASK;
        if ahead == 0 {
            return Water::pack(self.low().min(depth), self.owed(), self.interval());
        }
        if ahead >= 1 << (INTERVAL_BITS - 1) {
            // A clock that went back. By one interval it is most likely another CPU's clock a
            // little behind this word's writer's: stay in the word's interval, or the next
            // look from the writer's CPU would start it again and owe the whole low twice. By
            // more (a resume from suspend, a restored checkpoint), count the interval from the
            // clock's time now, so the next one begins an interval from now instead of when the
            // clock catches up.
            let back = self.interval().wrapping_sub(interval) & INTERVAL_MASK;
            let keep = if back <= 1 { self.interval() } else { interval };
            return Water::pack(self.low().min(depth), self.owed(), keep);
        }
        // Since the old mark was set the stack has not fallen below it, since every pop is
        // looked at, nor below its depth now. What was unpaid of the old debt lies in that
        // depth, so the new debt replaces it.
        Water::pack(depth, self.low().min(depth), interval)
    }

    /// What a note of the depth `depth` within the word's own interval makes of it: a new low,
    /// if it is one. A word no look has written stays so, for the first look to start.
    pub(super) fn lower(self, depth: u32) -> Water {
        if self.0 & SEEN == 0 {
            return self;
        }
        Water::pack(self.low().min(depth), self.owed(), self.interval())
    }

    /// The word after a shed laundered `objects`.
    pub(super) fn paid(self, objects: u32) -> Water {
        if self.0 & SEEN == 0 {
            return self;
        }
        Water::pack(
            self.low(),
            self.owed().saturating_sub(objects),
            self.interval(),
        )
    }
}

impl<O: Os, R: Routines, S: Source> Cache<'_, O, R, S> {
    /// Looks at the depth of `class`'s stack in `domain` at `now`: a new low, or the start of a
    /// new interval and what the stack owes for the last one.
    pub(super) fn look_at_depth(&self, domain: Domain, class: SmallClass, now: u64) {
        self.update_water(domain, class, Some(interval(now)), 0);
    }

    /// Notes the depth of `class`'s stack in `domain` after a pop, as a new low if it is one.
    /// Reads the clock only for a word no look has started yet, once per stack: otherwise a
    /// pop noted in an interval that has ended but not yet been started anew counts toward the
    /// old one, which can only make the stack shed less.
    pub(super) fn note_depth(&self, domain: Domain, class: SmallClass, clock: &Clock<'_, O>) {
        let ordinal = class.ordinal() % SMALL_COUNT;
        if Water(self.depot.water(domain, ordinal).load(Relaxed)).seen() {
            self.update_water(domain, class, None, 0);
        } else {
            self.look_at_depth(domain, class, clock.now());
        }
    }

    /// Pays `paid` objects off the debt of `class`'s stack in `domain`, then notes its depth,
    /// starting a new interval if `interval` says one has begun; returns the new word.
    fn update_water(
        &self,
        domain: Domain,
        class: SmallClass,
        interval: Option<u64>,
        paid: u32,
    ) -> Water {
        let ordinal = class.ordinal() % SMALL_COUNT;
        let word = self.depot.water(domain, ordinal);
        let mut seen = word.load(Relaxed);
        loop {
            let depth = self
                .depot
                .full(domain, class)
                .objects(self.area, class.magazine());
            let paid_off = Water(seen).paid(paid);
            let next = match interval {
                Some(interval) => paid_off.look(depth, interval),
                None => paid_off.lower(depth),
            };
            if next.0 == seen {
                return next;
            }
            // Sequentially consistent with the sets of owing classes and domains, so that a
            // shed that clears a bit and then reads this word cannot miss a debt written here
            // whose bit it cleared (see `settle`).
            match word.compare_exchange(seen, next.0, SeqCst, Relaxed) {
                Ok(_) => {
                    if next.owed() > 0 {
                        self.mark_owing(domain, ordinal);
                    }
                    return next;
                }
                Err(now) => seen = now,
            }
        }
    }

    /// Sets the owing bits of the stack of `ordinal` in `domain`, and of the domain.
    fn mark_owing(&self, domain: Domain, ordinal: usize) {
        let bit = 1u64 << ordinal;
        let owing = self.depot.owing(domain);
        if owing.load(SeqCst) & bit == 0 {
            owing.fetch_or(bit, SeqCst);
        }
        let domain_bit = 1u64 << domain.index();
        if self.depot.owing_domains.load(SeqCst) & domain_bit == 0 {
            self.depot.owing_domains.fetch_or(domain_bit, SeqCst);
        }
    }

    /// Launders one magazine from a stack that owes, if any does: in the first owing domain
    /// at or after `turn` (mod the domain bits), its first owing class at or after `turn`
    /// (mod the class count), so that CPUs spread over the domains and the classes.
    pub(super) fn shed(&self, turn: u64) {
        let domains = self.depot.owing_domains.load(Relaxed);
        if domains == 0 {
            return;
        }
        let start = (turn % u64::BITS as u64) as u32;
        let index = (domains.rotate_right(start).trailing_zeros() + start) % u64::BITS;
        let domain = Domain::folded(index as usize);
        let owing = self.depot.owing(domain).load(Relaxed);
        if owing == 0 {
            self.settle_domain(domain);
            return;
        }
        let start = (turn % SMALL_COUNT as u64) as u32;
        let ordinal = (owing.rotate_right(start).trailing_zeros() + start) % u64::BITS;
        self.shed_in(domain, ordinal as usize);
    }

    /// Launders one magazine from the stack of `ordinal` in `domain` if it owes, and settles
    /// its bit once its debt is paid.
    fn shed_in(&self, domain: Domain, ordinal: usize) {
        let Some(class) = SmallClass::from_ordinal(ordinal) else {
            // No look sets a bit past the classes; one that is set is cleared.
            self.depot
                .owing(domain)
                .fetch_and(!(1u64 << ordinal), SeqCst);
            return;
        };
        let owed = Water(self.depot.water(domain, ordinal).load(Relaxed)).owed();
        let paid = if owed == 0 {
            0
        } else {
            // An empty stack owes nothing more.
            self.launder_one(domain, class)
                .map_or(u32::MAX, |level| level as u32)
        };
        if self.update_water(domain, class, None, paid).owed() == 0 {
            self.settle(domain, ordinal);
        }
    }

    /// Pays every debt the stacks of every domain owe now, a magazine at a time, for
    /// `maintain`: what the heartbeats of an interval would launder by [`Cache::shed`]. Each
    /// shed of an owing stack pays at least one object or ends the debt, so a stack takes at
    /// most one shed more than the objects it owed when its turn came; the bound only keeps a
    /// debt that a racing look starts from extending the loop.
    pub(super) fn shed_all(&self) {
        for domain in self.depot.all_domains() {
            for class in SmallClass::all() {
                let ordinal = class.ordinal() % SMALL_COUNT;
                let bit = 1u64 << ordinal;
                let owed = Water(self.depot.water(domain, ordinal).load(Relaxed)).owed();
                for _ in 0..=owed {
                    if self.depot.owing(domain).load(Relaxed) & bit == 0 {
                        break;
                    }
                    self.shed_in(domain, ordinal);
                }
            }
        }
    }

    /// Clears the owing bit of a stack whose debt is paid. A look that made it owe again after
    /// the debt was read either set the bit after this clears it, or wrote its debt before the
    /// clear, in which case the read below sees it and the bit is set again. Then the domain's
    /// bit, if no class of it owes.
    fn settle(&self, domain: Domain, ordinal: usize) {
        let bit = 1u64 << ordinal;
        let owing = self.depot.owing(domain);
        owing.fetch_and(!bit, SeqCst);
        if Water(self.depot.water(domain, ordinal).load(SeqCst)).owed() > 0 {
            owing.fetch_or(bit, SeqCst);
        }
        if owing.load(SeqCst) == 0 {
            self.settle_domain(domain);
        }
    }

    /// Clears the owing bit of a domain none of whose stacks owes, as `settle` does a class's:
    /// a class bit set after the domain's was read is followed by a set of the domain's.
    fn settle_domain(&self, domain: Domain) {
        let bit = 1u64 << domain.index();
        self.depot.owing_domains.fetch_and(!bit, SeqCst);
        if self.depot.owing(domain).load(SeqCst) != 0 {
            self.depot.owing_domains.fetch_or(bit, SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::IDLE_TICKS;
    use super::*;

    #[test]
    fn a_first_look_owes_nothing_and_a_new_interval_owes_the_old_low() {
        let first = Water(0).look(40, 3);
        assert_eq!((first.low(), first.owed()), (40, 0));
        let lower = first.look(25, 3).look(31, 3);
        assert_eq!((lower.low(), lower.owed()), (25, 0));
        let next = lower.look(31, 4);
        assert_eq!((next.low(), next.owed()), (31, 25));
        // Several intervals with no look: the stack never fell below the old mark.
        let late = lower.look(60, 9);
        assert_eq!((late.low(), late.owed()), (60, 25));
        // Nor below its depth now, if that is lower.
        let drained = lower.look(10, 4);
        assert_eq!((drained.low(), drained.owed()), (10, 10));
    }

    #[test]
    fn paying_a_debt_and_wrapping() {
        let owing = Water(0).look(50, 1).look(50, 2);
        assert_eq!(owing.owed(), 50);
        assert_eq!(owing.paid(20).owed(), 30);
        assert_eq!(owing.paid(u32::MAX).owed(), 0);
        assert_eq!(Water(0).paid(5), Water(0));
        // The interval number wraps, and a clock that goes back stays in the same interval.
        let top = Water(0).look(7, INTERVAL_MASK);
        assert_eq!(top.look(7, 0).owed(), 7);
        assert_eq!(owing.look(3, 1).owed(), 50);
        assert_eq!(owing.look(3, 1).low(), 3);
        // A clock one interval behind (another CPU's) stays in the word's interval, so the
        // word's own CPU does not start that interval again.
        let behind = Water(0).look(9, 400).look(9, 401);
        assert_eq!(behind.look(4, 400).look(4, 401).owed(), 9);
        assert_eq!(behind.look(4, 400).look(4, 402).owed(), 4);
        // After a clock that went back further, the next interval begins one interval later.
        let back = Water(0).look(9, 400).look(9, 401).look(5, 3);
        assert_eq!((back.low(), back.owed()), (5, 9));
        assert_eq!(back.look(5, 3).owed(), 9);
        assert_eq!(back.look(6, 4).owed(), 5);
        // Counts saturate.
        let huge = Water(0).look(u32::MAX, 1);
        assert_eq!(huge.low(), COUNT_MAX);
        assert_eq!(huge.look(u32::MAX, 2).owed(), COUNT_MAX);
        assert_eq!(interval(5 * IDLE_TICKS + 1), 5);
    }
}
