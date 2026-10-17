//! A depot stack: a lock-free stack of magazines (`tla/Depot.tla`).
//!
//! The head is one 64-bit word: the top magazine's number in the low half and a version
//! counter in the high half, changed only by compare-and-swap, and the counter goes up by one
//! on every push and every pop. A magazine's link is its slot 0: the number of the magazine
//! below it and, in the high half, the number of objects in it and every magazine below, so the
//! top's link says how many objects the stack holds without a counter of its own. Magazine
//! memory is never unmapped, so a pop that reads the link of a magazine another thread has
//! already taken reads harmless memory, and its compare-and-swap then fails unless exactly a
//! multiple of `2^32` pushes and pops happened in between and left the same magazine on top.
//!
//! The all-zero head is an empty stack.

use super::magazine::{Area, Magazine, MagazineId};
use crate::classes::MagazineSize;
use crate::front::sync::AtomicU64;
use crate::os::Os;
use core::ptr;
use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};

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
pub struct Stack {
    head: AtomicU64,
}

impl Stack {
    /// An empty stack.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Stack {
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

    /// Pushes a magazine this thread holds; its recorded level counts toward the stack's
    /// objects. Afterwards the stack owns it.
    pub fn push<O: Os>(&self, area: Area<'_, O>, magazine: Magazine<'_>) {
        let level = magazine.level() as u32;
        let mut head = Packed::from_word(self.head.load(Acquire));
        loop {
            // The count below is read from the top's link, which may be stale; then the
            // compare-and-swap fails and the loop reads it again.
            let below = Self::link(area, head.number, magazine.size());
            let objects = if head.number == 0 { 0 } else { below.high };
            let link = Packed {
                number: head.number,
                high: objects.saturating_add(level),
            };
            magazine.link().store(
                ptr::without_provenance_mut(link.to_word() as usize),
                Release,
            );
            let new = Packed {
                number: magazine.id().get(),
                high: head.high.wrapping_add(1),
            };
            match self
                .head
                .compare_exchange(head.to_word(), new.to_word(), AcqRel, Acquire)
            {
                Ok(_) => return,
                Err(now) => head = Packed::from_word(now),
            }
        }
    }

    /// Pops a magazine of `size`, which this thread then holds.
    pub fn pop<'a, O: Os>(&self, area: Area<'a, O>, size: MagazineSize) -> Option<Magazine<'a>> {
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
                Ok(_) => return Some(magazine),
                Err(now) => head = Packed::from_word(now),
            }
        }
    }

    /// The number of objects in the stack, as of a moment during the call: the count in the
    /// top magazine's link. A hint for the depot's targets.
    pub fn objects<O: Os>(&self, area: Area<'_, O>, size: MagazineSize) -> u32 {
        let head = Packed::from_word(self.head.load(Relaxed));
        if head.number == 0 {
            0
        } else {
            Self::link(area, head.number, size).high
        }
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
