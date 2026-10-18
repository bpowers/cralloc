//! Grouping a launder's slots by bitmap word, without sorting them.
//!
//! A launder sets each word's bits with one atomic OR, so the fewer times it meets a word, the
//! fewer ORs. Magazines come back in a few shapes (`bench/backend` measures each): dense runs,
//! ascending or descending, from one or two words at a time, which is how a magazine filled
//! from the bitmaps is used and freed; and scattered, one slot to a word, when a program frees
//! in an order unrelated to the one it allocated in. A sort serves the first shape no better
//! than a few open words do, and the second not at all, at tens of cycles a slot.
//!
//! So a launder keeps the words it met most recently open, [`WAYS`] of them, with the bits of
//! the slots seen for each: one in the caller's hands ([`Open`]), the others here in
//! [`Group`]. A slot of the caller's word adds its bit there; a slot of another open word
//! swaps the two; any other slot opens its word and closes the oldest open one, which then
//! gets its OR. Two interleaved dense runs share the open words without closing either early.
//!
//! **Slots that appear twice.** A batch must not free a slot twice: the second free would undo
//! a claim made between the two ORs and hand the slot to two owners. A slot seen twice while its
//! word is open shows up in the open bits. A word that comes back after it was closed must also
//! remember what its earlier ORs set, so every closed word goes in a list with its bits, and in
//! a filter of one bit per word number modulo [`FILTER_BITS`]; a word that opens with its filter
//! bit set gathers its earlier bits from the list. The list is made afresh for every launder and
//! is short, [`LOG`] words, because clearing a list of 64 cost a dense launder a cycle a slot.
//! So a part ends when its list is full as well as after [`MAX_SLOTS`] slots: a part of dense
//! runs closes a few words in 64 slots, a scattered part one word a slot, and every part is
//! checked whole.

use super::tree::{Host, Release};
use super::word::BITS;

/// Words kept open at once: the caller's and `WAYS - 1` in the group.
pub(crate) const WAYS: usize = 4;

/// The most slots one part takes; a longer batch is grouped in parts, each checked for slots
/// that appear twice on its own. This is the chunk the depot launders in, so a chunk of dense
/// runs is one part.
pub(crate) const MAX_SLOTS: usize = 64;

/// Closed words a part's list holds; a part ends when it is full.
const LOG: usize = 16;

/// Bits in the filter of closed words.
const FILTER_BITS: usize = 1024;

/// A key no word has: region numbers are below 2^16.
const NONE: u64 = u64::MAX;

/// A bitmap word and the bits gathered for it. Two words, so that it travels in registers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Open {
    /// `region << 32 | word`, by number.
    pub(crate) key: u64,
    /// The bits of the slots seen since the word opened, for its OR.
    pub(crate) bits: u64,
}

impl Open {
    /// The word of slot `slot` of region `region`, with that slot's bit.
    #[inline(always)]
    pub(crate) fn slot(region: usize, slot: u32) -> Open {
        Open {
            key: (region as u64) << 32 | (slot / BITS) as u64,
            bits: 1 << (slot % BITS),
        }
    }

    /// Adds the bit of a slot of this word, `bit`. Aborts if the slot was seen since the word
    /// opened; one seen before that is caught when the word closes.
    #[inline(always)]
    pub(crate) fn add<H: Host>(&mut self, release: &Release<'_, H>, bit: u64) {
        if self.bits & bit != 0 {
            release.abort(TWICE);
        }
        self.bits |= bit;
    }
}

const TWICE: &str = "double free: an object was freed twice in one batch";

/// No word.
const CLOSED: Open = Open { key: NONE, bits: 0 };

/// The open words a launder is not holding, and the words it has closed: see the module
/// documentation.
///
/// Each open word also has the bits that earlier ORs into it set in this part, if it was
/// closed before and opened again: `earlier` for the words here, `held_earlier` for the
/// caller's. None of them may be among the word's bits when it closes.
pub(crate) struct Group {
    /// Open words, the most recently put back first; `NONE` keys when unused.
    ways: [Open; WAYS - 1],
    earlier: [u64; WAYS - 1],
    held_earlier: u64,
    /// Closed words by key, with the bits each OR set: `closed` of them.
    log: [Open; LOG],
    closed: usize,
    /// One bit per closed word's number modulo `FILTER_BITS`.
    filter: [u64; FILTER_BITS / 64],
}

impl Group {
    /// A group with no word open but the caller's, and none closed.
    #[inline]
    pub(crate) fn new() -> Group {
        Group {
            ways: [CLOSED; WAYS - 1],
            earlier: [0; WAYS - 1],
            held_earlier: 0,
            log: [CLOSED; LOG],
            closed: 0,
            filter: [0; FILTER_BITS / 64],
        }
    }

    /// The caller holds `held` open and has met `next`, one slot of another word. Returns the
    /// word the caller holds now. If `next`'s word is open here its bit is added here and the
    /// caller keeps `held`; otherwise `held` goes into the group, the oldest open word is
    /// closed into `release`, and the caller holds `next`. Aborts if `next`'s slot is already
    /// in the group.
    ///
    /// Inlined, with the open word found by a mask rather than a branch per way, and updated in
    /// place: a magazine whose slots come back shuffled among a few words, or two runs
    /// interleaved, meets an open word other than the held one on most slots.
    #[inline(always)]
    pub(crate) fn switch<H: Host>(
        &mut self,
        release: &mut Release<'_, H>,
        held: Open,
        next: Open,
    ) -> Open {
        let mut found = 0u32;
        for way in 0..WAYS - 1 {
            found |= ((self.ways[way].key == next.key) as u32) << way;
        }
        if found == 0 {
            return self.open(release, held, next);
        }
        let way = (found.trailing_zeros() as usize) % (WAYS - 1);
        if (self.ways[way].bits | self.earlier[way]) & next.bits != 0 {
            release.abort(TWICE);
        }
        self.ways[way].bits |= next.bits;
        held
    }

    /// [`Group::switch`] for a `next` whose word is not open: opens it and closes the oldest.
    #[inline(never)]
    fn open<H: Host>(&mut self, release: &mut Release<'_, H>, held: Open, next: Open) -> Open {
        let mut earlier = 0;
        let hash = key_hash(next.key);
        if self.filter[hash / 64] & (1 << (hash % 64)) != 0 {
            let closed = self.closed.min(LOG);
            for word in &self.log[..closed] {
                if word.key == next.key {
                    earlier |= word.bits;
                }
            }
            if earlier & next.bits != 0 {
                release.abort(TWICE);
            }
        }
        let (oldest, oldest_earlier) = (self.ways[WAYS - 2], self.earlier[WAYS - 2]);
        self.ways.copy_within(..WAYS - 2, 1);
        self.earlier.copy_within(..WAYS - 2, 1);
        self.ways[0] = held;
        self.earlier[0] = core::mem::replace(&mut self.held_earlier, earlier);
        if oldest.key != NONE {
            self.close(release, oldest, oldest_earlier);
        }
        next
    }

    /// Whether the part's list of closed words is full: the caller must finish the part before
    /// it meets another slot.
    #[inline(always)]
    pub(crate) fn is_full(&self) -> bool {
        self.closed >= LOG
    }

    /// Closes every open word, the caller's `held` among them, into `release`, and empties the
    /// group for another part.
    pub(crate) fn finish<H: Host>(&mut self, release: &mut Release<'_, H>, held: Open) {
        let held_earlier = core::mem::replace(&mut self.held_earlier, 0);
        self.close(release, held, held_earlier);
        for way in 0..WAYS - 1 {
            let open = core::mem::replace(&mut self.ways[way], CLOSED);
            let earlier = core::mem::replace(&mut self.earlier[way], 0);
            if open.key != NONE {
                self.close(release, open, earlier);
            }
        }
        self.closed = 0;
        self.filter = [0; FILTER_BITS / 64];
    }

    /// Sets a word's gathered bits with one atomic OR, and records it closed. `earlier` is
    /// what earlier ORs into the word set in this part.
    #[inline]
    fn close<H: Host>(&mut self, release: &mut Release<'_, H>, word: Open, earlier: u64) {
        if word.bits & earlier != 0 {
            release.abort(TWICE);
        }
        if let Some(entry) = self.log.get_mut(self.closed) {
            *entry = word;
            self.closed += 1;
        }
        let hash = key_hash(word.key);
        self.filter[hash / 64] |= 1 << (hash % 64);
        release.leaf_by_number((word.key >> 32) as usize, word.key as u32, word.bits);
    }
}

/// A closed word's place in the filter: its word and region numbers folded together, so that
/// neighbouring words, and the same word of neighbouring regions, have different places.
#[inline(always)]
fn key_hash(key: u64) -> usize {
    ((key ^ key >> 22) as usize) % FILTER_BITS
}
