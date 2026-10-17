//! What the simulated OS knows about each page: reserved, writable, purged, huge-page advice.

use crate::os::HugePages;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};

/// The page belongs to a reservation.
pub(super) const RESERVED: u8 = 1 << 0;
/// The page has been committed since it was reserved.
pub(super) const WRITABLE: u8 = 1 << 1;
/// The page has been purged since it was last committed.
pub(super) const PURGED: u8 = 1 << 2;
/// Advised for huge pages.
pub(super) const HUGE: u8 = 1 << 3;
/// Advised against huge pages.
pub(super) const NO_HUGE: u8 = 1 << 4;
/// The first page of a reservation, so that a range spanning two reservations can be told
/// from one inside a single reservation.
pub(super) const START: u8 = 1 << 5;
/// Marked to read as zero in a forked child.
pub(super) const WIPE: u8 = 1 << 6;

/// Per-page state, one atomic byte per page, so concurrent calls on different pages never
/// interfere and the simulator needs no lock.
pub(super) struct Pages {
    states: Box<[AtomicU8]>,
}

impl Pages {
    pub(super) fn new(count: usize) -> Pages {
        let mut states = Vec::with_capacity(count);
        states.resize_with(count, || AtomicU8::new(0));
        Pages {
            states: states.into_boxed_slice(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.states.len()
    }

    pub(super) fn get(&self, page: usize) -> u8 {
        self.states[page].load(Ordering::Acquire)
    }

    /// Marks `first..first + count` reserved if none of them is, else changes nothing and
    /// returns false. Each page is claimed by compare-and-swap and released again if a later
    /// one is taken, so racing reservations never overlap.
    pub(super) fn claim(&self, first: usize, count: usize) -> bool {
        for page in first..first + count {
            let state = if page == first {
                RESERVED | START
            } else {
                RESERVED
            };
            let claimed = self.states[page]
                .compare_exchange(0, state, Ordering::AcqRel, Ordering::Acquire)
                .is_ok();
            if !claimed {
                self.release(first, page - first);
                return false;
            }
        }
        true
    }

    /// Forgets everything about `first..first + count`. Release, so that a later claim sees the
    /// zeroing that preceded it.
    pub(super) fn release(&self, first: usize, count: usize) {
        for state in &self.states[first..first + count] {
            state.store(0, Ordering::Release);
        }
    }

    /// Sets `set` and clears `clear` on every page of `first..first + count`.
    pub(super) fn update(&self, first: usize, count: usize, set: u8, clear: u8) {
        for state in &self.states[first..first + count] {
            let mut old = state.load(Ordering::Relaxed);
            while let Err(now) = state.compare_exchange_weak(
                old,
                (old & !clear) | set,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                old = now;
            }
        }
    }

    /// Whether every page of `first..first + count` has all the bits of `mask`.
    pub(super) fn all(&self, first: usize, count: usize, mask: u8) -> bool {
        self.states[first..first + count]
            .iter()
            .all(|state| state.load(Ordering::Acquire) & mask == mask)
    }

    /// Marks `page` as the first of a reservation if it is reserved and not already marked.
    /// A compare-and-swap that requires `RESERVED`, so a page another thread releases
    /// meanwhile stays free rather than gaining a stray `START`.
    pub(super) fn mark_start(&self, page: usize) {
        let state = &self.states[page];
        let mut old = state.load(Ordering::Acquire);
        while old & RESERVED != 0 && old & START == 0 {
            match state.compare_exchange_weak(old, old | START, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return,
                Err(now) => old = now,
            }
        }
    }

    /// Whether some page of `first..first + count` has any bit of `mask`.
    pub(super) fn any(&self, first: usize, count: usize, mask: u8) -> bool {
        self.states[first..first + count]
            .iter()
            .any(|state| state.load(Ordering::Acquire) & mask != 0)
    }

    /// The pages, in order, with all the bits of `mask`.
    pub(super) fn with(&self, mask: u8) -> impl Iterator<Item = usize> + '_ {
        (0..self.states.len()).filter(move |&page| self.get(page) & mask == mask)
    }

    /// The number of pages with all the bits of `mask`.
    pub(super) fn count(&self, mask: u8) -> usize {
        self.states
            .iter()
            .filter(|state| state.load(Ordering::Acquire) & mask == mask)
            .count()
    }
}

/// The huge-page advice a page state records.
pub(super) fn advice(state: u8) -> Option<HugePages> {
    if state & HUGE != 0 {
        Some(HugePages::Prefer)
    } else if state & NO_HUGE != 0 {
        Some(HugePages::Avoid)
    } else {
        None
    }
}
