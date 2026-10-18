//! Grouping a launder's slots by bitmap word, without sorting them.
//!
//! A launder sets each word's bits with one atomic OR, so the fewer times it meets a word, the
//! fewer ORs. Magazines come back in a few shapes (`bench/backend` measures each): dense runs,
//! ascending or descending, from one or two words at a time, which is how a magazine filled
//! from the bitmaps is used and freed; slots shuffled among a few neighbouring words; and
//! scattered, one slot to a word, when a program frees in an order unrelated to the one it
//! allocated in. Sorting a magazine costs some twenty cycles a slot, as much as the rest of a
//! scattered launder.
//!
//! So the caller gathers a run of consecutive slots of one word in hand ([`Run`]), and when the
//! run ends it goes to [`Gather`]: into a window of one or two summary words' leaf words
//! around the batch's first slot, where its bits are ORed into a plain array; or, outside the
//! window, into a small hash map of the batch's words ([`Words`]), which merges them with the
//! bits the word already has there. A batch whose runs are single slots stops making runs: its
//! slots go one at a time to the window, or, if they mostly fall outside it, straight to the
//! map, so that the processor is not asked to guess, slot by slot, which of the two a
//! scattered slot falls in. At the end every word met gets one OR, the window's in address
//! order, and the words under one summary word owe that summary word one debt between them
//! ([`Release::owe_leaves`]), so the walk up the tree looks at each summary word once, however
//! the words were met.
//!
//! **Slots that appear twice.** A batch must not free a slot twice: the second free would undo
//! a claim made between the two ORs and hand the slot to two owners. A run counts its slots
//! against its bits, and the window and the map hold every word the batch has met with all of
//! its bits until the end, a word in one or the other but never both, so a slot met twice is
//! caught when it comes the second time. The map holds `ROOM - 1` words, more than a magazine
//! has slots; a longer batch flushes it when it fills, and a slot that appears on both sides of
//! a flush is caught by its word's OR unless another thread claimed the slot in between, as for
//! any two launders.

use super::tree::{Host, Release};
use super::word::BITS;
use crate::space::RegionId;

const TWICE: &str = "double free: an object was freed twice in one batch";

/// A run of slots of one word: the word's key (`region << 32 | word`), the bits of its slots,
/// and how many slots it took. A slot taken twice sets its bit once, so the count and the bits
/// disagree; that is checked when the run ends ([`check_count`]), rather than bit by bit.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Run {
    pub(crate) key: u64,
    pub(crate) bits: u64,
    pub(crate) taken: u32,
}

/// Where a launder's runs go when they end: see the module documentation.
///
/// A window of `SPAN` leaf words, one or two summary words aligned to a summary word, placed on the
/// batch's first slot: a run of a word inside it is ORed into the word's bits there, with one
/// look for a slot given twice. Runs of any other word go to the map, which is made only when
/// the first one comes. A dense magazine, two interleaved runs and slots shuffled among a few
/// words all fall in the window, which costs a few instructions a slot and holds the bits in
/// address order for the flush; a scattered magazine mostly does not.
pub(crate) struct Gather<const ROOM: usize, const SPAN: usize> {
    /// The window's region's number, shifted as in a key, and its first leaf word.
    region: u64,
    base: u64,
    /// The bits gathered for each leaf word of the window.
    bits: [u64; SPAN],
    /// Per summary word of the window, a bit for each of its leaf words that has bits: what
    /// the summary word is owed, and which words the flush ORs, found without a branch per
    /// word, which for a few words scattered in the window the processor would mispredict.
    /// `SPAN` is at most two summary words.
    occupied: [u64; 2],
    words: Option<Words<ROOM>>,
}

impl<const ROOM: usize, const SPAN: usize> Gather<ROOM, SPAN> {
    /// A window on the summary words around word `key` (`region << 32 | word`), the batch's
    /// first: the word's own and the nearer of its neighbours.
    #[inline(always)]
    pub(crate) fn new(key: u64) -> Gather<ROOM, SPAN> {
        let word = key as u32 as u64;
        Gather {
            region: key & !(u32::MAX as u64),
            // With two summary words, the one the word is nearer the edge of, and its own.
            base: word.saturating_sub((SPAN as u64 - BITS as u64) / 4) & !(BITS as u64 - 1),
            bits: [0; SPAN],
            occupied: [0; 2],
            words: None,
        }
    }

    /// A run ends. Aborts if one of its slots was given twice: twice in the run, or once in it
    /// and once before.
    #[inline(always)]
    pub(crate) fn end<H: Host>(&mut self, release: &mut Release<'_, H>, run: Run) {
        check_count(release, run);
        self.end_one(release, run.key, run.bits);
    }

    /// For the rest of a batch of single slots: if they have mostly fallen outside the window,
    /// moves the window's words to the map, so that every later slot goes there through
    /// [`Gather::scattered`] without asking whether it falls in the window, which for scattered
    /// slots is a coin toss the processor cannot predict, and returns `true`. Slots that keep
    /// to the window, as two interleaved runs or a few shuffled words do, stay with it.
    #[inline(never)]
    pub(crate) fn scatter<H: Host>(&mut self, release: &mut Release<'_, H>) -> bool {
        let mapped = self.words.as_ref().map_or(0, |words| words.count);
        let windowed = self
            .occupied
            .iter()
            .map(|o| o.count_ones() as usize)
            .sum::<usize>();
        if mapped <= windowed {
            return false;
        }
        let words = self.words.get_or_insert_with(Words::new);
        for half in 0..2 {
            let mut occupied = core::mem::replace(&mut self.occupied[half], 0);
            while occupied != 0 {
                let at = half * BITS as usize + occupied.trailing_zeros() as usize;
                occupied &= occupied - 1;
                let bits = core::mem::replace(&mut self.bits[at % SPAN], 0);
                words.add(release, self.region | (self.base + at as u64), bits);
            }
        }
        true
    }

    /// One slot of a scattered batch, after [`Gather::scatter`]: bit `bit` of word `key`.
    #[inline(always)]
    pub(crate) fn scattered<H: Host>(&mut self, release: &mut Release<'_, H>, key: u64, bit: u64) {
        match self.words.as_mut() {
            Some(words) => words.add(release, key, bit),
            None => release.abort("a scattered launder with no map"),
        }
    }

    /// [`Gather::end`] for bits `bits` of word `key` whose run is checked, or which is one
    /// slot.
    #[inline(always)]
    pub(crate) fn end_one<H: Host>(&mut self, release: &mut Release<'_, H>, key: u64, bits: u64) {
        let at = (key ^ self.region).wrapping_sub(self.base);
        if at < SPAN as u64 {
            let at = at as usize % SPAN;
            if self.bits[at] & bits != 0 {
                release.abort(TWICE);
            }
            self.bits[at] |= bits;
            self.occupied[at / BITS as usize % 2] |= 1 << (at % BITS as usize);
            return;
        }
        self.words
            .get_or_insert_with(Words::new)
            .add(release, key, bits);
    }

    /// ORs every word gathered into `release`: the window's in address order, owing each of
    /// its summary words one debt; then the map's.
    #[inline(never)]
    pub(crate) fn finish<H: Host>(&mut self, release: &mut Release<'_, H>) {
        if self.occupied != [0; 2] {
            let region = release.region((self.region >> 32) as usize);
            for half in 0..2 {
                let below = self.occupied[half];
                let mut occupied = below;
                let mut must = false;
                while occupied != 0 {
                    let at = half * BITS as usize + occupied.trailing_zeros() as usize;
                    occupied &= occupied - 1;
                    let index = (self.base as usize + at) as u32;
                    must |= release.leaf_unowed(region, index, self.bits[at % SPAN]);
                }
                if below != 0 {
                    let first = (self.base as usize + half * BITS as usize) as u32;
                    release.owe_leaves(region, first, below, must);
                }
            }
        }
        if let Some(words) = self.words.as_mut() {
            words.flush(release);
        }
    }
}

/// Aborts unless `run` has a bit for each of its slots. A run of one slot has one bit by
/// construction; this spares a scattered launder the count, which without `popcnt` is a dozen
/// instructions.
#[inline(always)]
fn check_count<H: Host>(release: &Release<'_, H>, run: Run) {
    if run.taken > 1 && run.bits.count_ones() != run.taken {
        release.abort(TWICE);
    }
}

/// The map of words for a batch of up to `ROOM - 1` words, made afresh for each launder on the
/// caller's stack: about 18 bytes a word, all zeroed when it is made. `ROOM` is a power of two
/// from 2 to 256, so that an entry's number fits a byte and indexing needs no division; a
/// launder picks it from the batch's length.
pub(crate) struct Words<const ROOM: usize> {
    /// The table of places, `2 * ROOM` of them, a hash of a word's key choosing the first it
    /// tries: one more than the entry of the word placed there, or zero for none.
    places: [[u8; 2]; ROOM],
    /// The words met: `region << 32 | word`, and the bits gathered for it.
    keys: [u64; ROOM],
    bits: [u64; ROOM],
    count: usize,
}

impl<const ROOM: usize> Words<ROOM> {
    /// A map with no words.
    #[inline(always)]
    pub(crate) fn new() -> Words<ROOM> {
        Words {
            places: [[0; 2]; ROOM],
            keys: [0; ROOM],
            bits: [0; ROOM],
            count: 0,
        }
    }

    /// Adds bits `bits` of word `key` (`region << 32 | word`), merging them with what the map
    /// has for that word. Aborts if a slot is in both. Flushes the map into `release` if it
    /// fills.
    #[inline(always)]
    fn add<H: Host>(&mut self, release: &mut Release<'_, H>, key: u64, bits: u64) {
        let places = 2 * ROOM;
        let mut place = (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40) as usize;
        loop {
            let at = place % places;
            let entry = self.places[at / 2 % ROOM][at % 2] as usize;
            if entry == 0 {
                let new = self.count % ROOM;
                self.keys[new] = key;
                self.bits[new] = bits;
                self.count += 1;
                self.places[at / 2 % ROOM][at % 2] = self.count as u8;
                if self.count >= ROOM - 1 {
                    self.flush(release);
                }
                return;
            }
            let met = (entry - 1) % ROOM;
            if self.keys[met] == key {
                if self.bits[met] & bits != 0 {
                    release.abort(TWICE);
                }
                self.bits[met] |= bits;
                return;
            }
            place += 1;
        }
    }

    /// Sets every word's bits with one atomic OR each, into `release`, and empties the map. The
    /// words under one summary word go one after another and owe it one debt
    /// ([`Release::owe_leaves`]); a summary word that the grouping could not place (see
    /// [`Groups`]) has more than one group, each owing its own.
    #[inline(never)]
    pub(crate) fn flush<H: Host>(&mut self, release: &mut Release<'_, H>) {
        let count = self.count.min(ROOM - 1);
        let mut groups = Groups::<ROOM>::new();
        for (at, &key) in self.keys[..count].iter().enumerate() {
            groups.add(at, key);
        }
        let mut region: Option<(u64, RegionId)> = None;
        for group in 0..groups.count.min(ROOM) {
            let summary = groups.summaries[group];
            let number = summary >> 26;
            let region = match region {
                Some((n, id)) if n == number => id,
                _ => {
                    let id = release.region(number as usize);
                    region = Some((number, id));
                    id
                }
            };
            let (mut below, mut must) = (0u64, false);
            let mut at = groups.first[group] as usize;
            loop {
                let index = self.keys[at % ROOM] as u32;
                must |= release.leaf_unowed(region, index, self.bits[at % ROOM]);
                below |= 1 << (index % BITS);
                match groups.next[at % ROOM] {
                    0 => break,
                    next => at = next as usize - 1,
                }
            }
            let index = (summary as u32 & ((1 << 26) - 1)) * BITS;
            release.owe_leaves(region, index, below, must);
        }
        self.count = 0;
        self.places = [[0; 2]; ROOM];
    }
}

/// Places in the table that finds a summary word's group, by the summary word's low bits:
/// neighbouring summary words, which a batch's words mostly sit under, take different places.
/// A word whose place holds another summary word starts a group of its own, so a batch spread
/// wider owes more debts, never fewer or more ORs.
const PLACES: usize = 64;

/// The map's words linked into groups by the summary word above them, for [`Words::flush`]: a
/// word's summary word is its key over 64 (`region << 26 | word / 64`).
struct Groups<const ROOM: usize> {
    /// Per place, one more than the group last made for a summary word there, or zero.
    places: [u8; PLACES],
    summaries: [u64; ROOM],
    /// Per group, its first word, and its last, which the next word of the group links from.
    first: [u8; ROOM],
    last: [u8; ROOM],
    /// Per word, one more than the next word of its group, or zero for the last.
    next: [u8; ROOM],
    count: usize,
}

impl<const ROOM: usize> Groups<ROOM> {
    #[inline(always)]
    fn new() -> Groups<ROOM> {
        Groups {
            places: [0; PLACES],
            summaries: [0; ROOM],
            first: [0; ROOM],
            last: [0; ROOM],
            next: [0; ROOM],
            count: 0,
        }
    }

    /// Links word `at`, whose key is `key`, into its group, or starts one.
    #[inline(always)]
    fn add(&mut self, at: usize, key: u64) {
        let summary = key >> 6;
        let place = summary as usize % PLACES;
        let seen = self.places[place] as usize;
        if seen != 0 {
            let group = (seen - 1) % ROOM;
            if self.summaries[group] == summary {
                self.next[self.last[group] as usize % ROOM] = at as u8 + 1;
                self.last[group] = at as u8;
                return;
            }
        }
        let group = self.count % ROOM;
        self.summaries[group] = summary;
        self.first[group] = at as u8;
        self.last[group] = at as u8;
        self.count += 1;
        self.places[place] = self.count as u8;
    }
}
