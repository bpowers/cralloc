//! A depot stack: a lock-free stack of magazines (`tla/Depot.tla`).
//!
//! The head is one 64-bit word: the top magazine's number in the low half and a version
//! counter in the high half, changed only by compare-and-swap, and the counter goes up by one
//! on every push and every pop. A magazine's link is its slot 0: the number of the magazine
//! below it and, in the high half, the stack's count up to and including it, so the top's link
//! gives the count without a counter of its own. Each push says what its magazine adds to the
//! count: a stack of magazines with objects counts objects, and a stack of empty ones counts
//! magazines. Magazine memory is never unmapped, so a pop that reads the link of a magazine
//! another thread has already taken reads harmless memory, and its compare-and-swap then fails
//! unless exactly a multiple of `2^32` pushes and pops happened in between and left the same
//! magazine on top.
//!
//! **A capped push** gives the magazine back instead when the stack's count has reached a cap.
//! The cap is exact: the count is read from the top's link between reading the head and the
//! compare-and-swap, and the compare-and-swap succeeds only if the head, version and all, is
//! still the one read, so the top is still the magazine whose link was read and its link is
//! the one its pusher wrote. A push that gives up has written nothing anyone else reads: at most
//! the link of the magazine it still holds, from a try whose compare-and-swap failed.
//!
//! The all-zero head is an empty stack.

use super::magazine::{Area, Magazine, MagazineId};
use crate::classes::MagazineSize;
use crate::front::sync::AtomicU64;
use crate::os::Os;
use core::ptr;
use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release, SeqCst};

/// What a push did besides taking the magazine.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Pushed {
    /// The stack was empty before it.
    pub(crate) onto_empty: bool,
    /// A compare-and-swap of the push failed: another thread changed the head meanwhile.
    pub(crate) contended: bool,
}

/// A head or a link: a magazine number (zero for none) and a version or object count.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Packed {
    pub(super) number: u32,
    pub(super) high: u32,
}

impl Packed {
    pub(super) const fn from_word(word: u64) -> Packed {
        Packed {
            number: word as u32,
            high: (word >> 32) as u32,
        }
    }

    pub(super) const fn to_word(self) -> u64 {
        (self.high as u64) << 32 | self.number as u64
    }
}

/// A stack of magazines of one size. See the module documentation.
pub(crate) struct Stack {
    head: AtomicU64,
}

impl Stack {
    /// An empty stack.
    #[allow(clippy::new_without_default)]
    pub(crate) const fn new() -> Stack {
        Stack {
            head: AtomicU64::new(0),
        }
    }

    fn link<O: Os>(area: Area<'_, O>, number: u32, size: MagazineSize) -> Packed {
        let below = MagazineId::new(number).and_then(|id| area.by_id(id, size));
        match below {
            Some(magazine) => Packed::from_word(magazine.link().load(Acquire).addr() as u64),
            None => Packed::from_word(0),
        }
    }

    /// Pushes a magazine this thread holds, which adds `weight` to the stack's count; afterwards
    /// the stack owns it.
    /// Whether a compare-and-swap of the push failed.
    pub(crate) fn push<O: Os>(
        &self,
        area: Area<'_, O>,
        magazine: Magazine<'_>,
        weight: u32,
    ) -> bool {
        // Without a cap the push cannot give the magazine back.
        self.push_under(area, magazine, weight, u32::MAX)
            .is_ok_and(|pushed| pushed.contended)
    }

    /// Pushes a magazine this thread holds, which adds `weight` to the stack's count, if the
    /// count is below `cap`; afterwards the stack owns it. Otherwise gives it back. Whether
    /// the stack was empty before the push.
    pub(crate) fn push_under<'a, O: Os>(
        &self,
        area: Area<'_, O>,
        magazine: Magazine<'a>,
        weight: u32,
        cap: u32,
    ) -> Result<Pushed, Magazine<'a>> {
        let mut contended = false;
        let mut head = Packed::from_word(self.head.load(Acquire));
        loop {
            // The count below is read from the top's link, which may be stale; then the
            // compare-and-swap fails and the loop reads it again.
            let below = Self::link(area, head.number, magazine.size());
            let count = if head.number == 0 { 0 } else { below.high };
            if count >= cap {
                return Err(magazine);
            }
            let link = Packed {
                number: head.number,
                high: count.saturating_add(weight),
            };
            magazine.link().store(
                ptr::without_provenance_mut(link.to_word() as usize),
                Release,
            );
            let new = Packed {
                number: area.id(&magazine).get(),
                high: head.high.wrapping_add(1),
            };
            // Sequentially consistent so that the hint of which stacks hold magazines, which
            // a push onto an empty stack sets after this and a look clears before reading the
            // head, is never left clear on a stack that holds one (`held.rs`). On x86-64 every
            // compare-and-swap is.
            match self
                .head
                .compare_exchange(head.to_word(), new.to_word(), SeqCst, Acquire)
            {
                Ok(_) => {
                    return Ok(Pushed {
                        onto_empty: head.number == 0,
                        contended,
                    });
                }
                Err(now) => {
                    contended = true;
                    head = Packed::from_word(now);
                }
            }
        }
    }

    /// Pops a magazine of `size`, which this thread then holds.
    pub(crate) fn pop<'a, O: Os>(
        &self,
        area: Area<'a, O>,
        size: MagazineSize,
    ) -> Option<Magazine<'a>> {
        self.pop_noting(area, size).map(|(magazine, _)| magazine)
    }

    /// Pops a magazine of `size`, which this thread then holds, and says whether a
    /// compare-and-swap of the pop failed: whether the stack was contended.
    pub(crate) fn pop_noting<'a, O: Os>(
        &self,
        area: Area<'a, O>,
        size: MagazineSize,
    ) -> Option<(Magazine<'a>, bool)> {
        let mut contended = false;
        let mut head = Packed::from_word(self.head.load(Acquire));
        loop {
            let top = MagazineId::new(head.number)?;
            let Some(magazine) = area.by_id(top, size) else {
                area.space()
                    .os()
                    .abort("a depot stack names something that is not a magazine");
            };
            let next = Packed::from_word(magazine.link().load(Acquire).addr() as u64);
            let new = Packed {
                number: next.number,
                high: head.high.wrapping_add(1),
            };
            match self
                .head
                .compare_exchange(head.to_word(), new.to_word(), AcqRel, Acquire)
            {
                Ok(_) => return Some((magazine, contended)),
                Err(now) => {
                    contended = true;
                    head = Packed::from_word(now);
                }
            }
        }
    }

    /// The stack's count, as of a moment during the call: the count in the top magazine's
    /// link, objects or magazines as its pushes counted. A hint for the depot's targets.
    pub(crate) fn count<O: Os>(&self, area: Area<'_, O>, size: MagazineSize) -> u32 {
        let head = Packed::from_word(self.head.load(Relaxed));
        if head.number == 0 {
            0
        } else {
            Self::link(area, head.number, size).high
        }
    }

    /// Whether the stack holds a magazine, read sequentially consistently, for the hint that
    /// a look clears and a push sets (`held.rs`).
    pub(crate) fn holds_any(&self) -> bool {
        Packed::from_word(self.head.load(SeqCst)).number != 0
    }

    /// Calls `visit` for each magazine on the stack, top first, without taking any, for at most
    /// `limit` of them. For the introspection walk: beside pushes and pops it may skip, repeat
    /// or stop early, but every magazine it visits is one the area admits.
    pub(super) fn walk<'a, O: Os>(
        &self,
        area: Area<'a, O>,
        size: MagazineSize,
        limit: usize,
        mut visit: impl FnMut(&Magazine<'a>),
    ) {
        let mut number = Packed::from_word(self.head.load(Relaxed)).number;
        for _ in 0..limit {
            let Some(magazine) = MagazineId::new(number).and_then(|id| area.by_id(id, size)) else {
                return;
            };
            number = Packed::from_word(magazine.link().load(Relaxed).addr() as u64).number;
            visit(&magazine);
        }
    }

    /// The head's version: how many pushes and pops the stack has had, modulo `2^32`. For tests
    /// that check a stack was not touched.
    #[cfg(test)]
    pub(crate) fn version(&self) -> u32 {
        Packed::from_word(self.head.load(Relaxed)).high
    }

    /// Every magazine on the stack, top first, without taking any. For audits of a quiescent
    /// heap; panics on a loop.
    #[cfg(test)]
    pub(crate) fn magazines<'a, O: Os>(
        &self,
        area: Area<'a, O>,
        size: MagazineSize,
    ) -> std::vec::Vec<Magazine<'a>> {
        let mut found = std::vec::Vec::new();
        let mut number = Packed::from_word(self.head.load(Relaxed)).number;
        while let Some(id) = MagazineId::new(number) {
            let magazine = area.by_id(id, size).expect("a stack names a magazine");
            assert!(found.len() < 1 << 20, "a depot stack loops");
            number = Packed::from_word(magazine.link().load(Relaxed).addr() as u64).number;
            found.push(magazine);
        }
        found
    }
}
