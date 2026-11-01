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
//! window, among a few early words and then into a small hash map of the batch's words
//! ([`Words`]), which merges them with the bits the word already has there. At the end every
//! word met gets one OR, the window's in address order, and the words under one summary word
//! owe that summary word one debt between them ([`Release::owe_leaves`]), so the walk up the
//! tree looks at each summary word once, however the words were met.
//!
//! A batch whose runs are single slots stops making runs. If its words have mostly fallen
//! inside the window, as two interleaved runs or a few shuffled words do, its slots go one at
//! a time to the window or the map. If they have mostly fallen outside, the batch is
//! scattered, and the rest of it is freed a slot at a time ([`Direct`]): an OR of its own for
//! each slot, which the slot would mostly have needed anyway, and no map. The judgement is made
//! once per batch, after a few single slots, so a batch is never taken one way and then the
//! other.
//!
//! **Slots that appear twice.** A batch must not free a slot twice: the second free would undo
//! a claim made between the two ORs and hand the slot to two owners. A run counts its slots
//! against its bits, and the window and the map hold every word the batch has met with all of
//! its bits until the end, a word in one or the other but never both, so a slot met twice is
//! caught when it comes the second time. The map holds `ROOM - 1` words, at most 63, and a batch
//! that meets more flushes it when it fills: the map lives on the stack of whatever thread is
//! freeing, which may be small. A slot that appears on both sides of a flush, or twice in a
//! scattered batch, is caught by its second OR unless another thread claimed the slot in
//! between: the same double free that goes unseen across two launders, and not worth a lookup
//! for every scattered slot.

use super::tree::{Host, Levels, Release, Upward};
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
    /// The first words met outside the window, before there are enough to need the map:
    /// a scattered batch is judged one before it has met more, and then needs no map at all.
    early: [(u64, u64); EARLY],
    early_count: usize,
    spill: Spill<ROOM>,
}

/// Where a batch's words go beyond the window and the early words: the map, or, once the batch
/// is judged scattered, the direct path. The two are never needed at once, and share one place
/// on the stack of whatever thread is freeing, which may be small.
// The variants differ in size and both live on the stack: there is no heap to box one on, and
// the enum is the one place the two share.
#[allow(clippy::large_enum_variant)]
enum Spill<const ROOM: usize> {
    Nothing,
    Map(Words<ROOM>),
    Direct(Direct),
}

/// Words outside the window that [`Gather`] holds before it makes its map: as many as the
/// single slots after which a batch is judged scattered.
const EARLY: usize = 8;

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
            early: [(0, 0); EARLY],
            early_count: 0,
            spill: Spill::Nothing,
        }
    }

    /// A run ends. Aborts if one of its slots was given twice: twice in the run, or once in it
    /// and once before.
    #[inline(always)]
    pub(crate) fn end<H: Host>(&mut self, release: &mut Release<'_, H>, run: Run) {
        check_count(release, run);
        self.end_one(release, run.key, run.bits);
    }

    /// Whether a batch whose last runs were single slots is scattered: its words have mostly
    /// fallen outside the window. Slots that keep to the window, as two interleaved runs or a
    /// few shuffled words do, are not.
    #[inline(always)]
    pub(crate) fn scattered(&self) -> bool {
        let mapped = self.early_count
            + match &self.spill {
                Spill::Map(words) => words.count,
                _ => 0,
            };
        let windowed = self
            .occupied
            .iter()
            .map(|o| o.count_ones() as usize)
            .sum::<usize>();
        mapped > windowed
    }

    /// Turns the batch to the direct path: ORs the map's words, if there is a map, puts a
    /// [`Direct`] in its place, hands it every word gathered in the window and the early words,
    /// and returns it for the rest of the batch. [`Gather::finish`] pays its debts.
    #[inline(never)]
    pub(crate) fn go_direct<H: Host>(&mut self, release: &mut Release<'_, H>) -> &mut Direct {
        if let Spill::Map(words) = &mut self.spill {
            words.flush(release);
        }
        self.spill = Spill::Direct(Direct::new());
        let Spill::Direct(direct) = &mut self.spill else {
            release.abort("a launder lost its direct path");
        };
        if self.occupied != [0; 2] {
            let region = release.region((self.region >> 32) as usize);
            for half in 0..2 {
                let mut occupied = core::mem::replace(&mut self.occupied[half], 0);
                while occupied != 0 {
                    let at = half * BITS as usize + occupied.trailing_zeros() as usize;
                    occupied &= occupied - 1;
                    let bits = core::mem::replace(&mut self.bits[at % SPAN], 0);
                    direct.free(release, region, (self.base as usize + at) as u32, bits);
                }
            }
        }
        let early = core::mem::replace(&mut self.early_count, 0);
        for &(key, bits) in &self.early[..early.min(EARLY)] {
            let region = release.region((key >> 32) as usize);
            direct.free(release, region, key as u32, bits);
        }
        direct
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
        self.outside(release, key, bits);
    }

    /// [`Gather::end_one`] for a word outside the window: among the early words while they
    /// have room, else in the map.
    #[inline(never)]
    fn outside<H: Host>(&mut self, release: &mut Release<'_, H>, key: u64, bits: u64) {
        match &mut self.spill {
            Spill::Map(words) => return words.add(release, key, bits),
            Spill::Direct(_) => release.abort("a gathered run after the direct path began"),
            Spill::Nothing => {}
        }
        let count = self.early_count.min(EARLY);
        if let Some(early) = self.early[..count].iter_mut().find(|(k, _)| *k == key) {
            if early.1 & bits != 0 {
                release.abort(TWICE);
            }
            early.1 |= bits;
            return;
        }
        if count < EARLY {
            self.early[count] = (key, bits);
            self.early_count += 1;
            return;
        }
        self.spill = Spill::Map(Words::new());
        let Spill::Map(words) = &mut self.spill else {
            release.abort("a launder lost its map");
        };
        for &(key, bits) in &self.early {
            words.add(release, key, bits);
        }
        self.early_count = 0;
        words.add(release, key, bits);
    }

    /// ORs every word gathered into `release`: the window's in address order, owing each of
    /// its summary words one debt; then the map's. Or pays the direct path's debts.
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
        let early = core::mem::replace(&mut self.early_count, 0);
        for &(key, bits) in &self.early[..early.min(EARLY)] {
            let region = release.region((key >> 32) as usize);
            let index = key as u32;
            let must = release.leaf_unowed(region, index, bits);
            release.owe_leaves(region, index, 1 << (index % BITS), must);
        }
        match &mut self.spill {
            Spill::Map(words) => words.flush(release),
            Spill::Direct(direct) => direct.finish(release),
            Spill::Nothing => {}
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

/// A scattered batch, freed a slot at a time: see the module documentation.
///
/// Each slot is ORed into its leaf word on its own, one locked instruction whose old value
/// says whether the slot was free already, which is a double free. Grouping slots by word
/// first would save nothing, as each has a word of its own, and costs about as much as the OR.
/// What the words owe the summary words above them is kept in [`Debts`], which merges the
/// debts of words under one summary word, as the map's groups do.
///
/// The OR's old value is not asked whether the word was empty: each debt is paid by reading
/// the summary word and setting only the bits it lacks (`Look` in `tla/Summary.tla`), which in
/// a scattered batch, whose words share summary words with many others, is a plain load where
/// setting the bits unread would be a locked OR.
///
/// **Wide batches.** Merging pays only while the batch's slots share summary words. A batch
/// spread over more summary words than it has slots, as the slots of a class with a few
/// million live objects are, shares almost none, and then the table costs a place taken and
/// another's debt paid for every slot. So a batch whose first [`JUDGE`] slots ([`Direct::free_slot`])
/// have nearly all taken places of their own is judged wide ([`Direct::wide`]), once, and each
/// later slot makes its walk up the region's tree at once ([`Direct::free_slot_wide`]): a load
/// and a test at each level, the words held in the cache by the slots before it near the top,
/// and missed only where the batch is truly spread.
pub(crate) struct Direct {
    debts: Debts,
}

/// Slots of a scattered batch after which it is judged wide or not.
pub(crate) const JUDGE: usize = 64;
/// Of those, places taken in the table above which the batch is wide: its slots fell in nearly
/// as many summary words as there are slots, as they do when the summary words outnumber them
/// several times. Slots that come back to summary words they met before, as strided or
/// clustered ones do, take fewer.
const WIDE: u32 = 56;

impl Direct {
    #[inline(always)]
    pub(crate) fn new() -> Direct {
        Direct {
            debts: Debts::new(),
        }
    }

    /// Whether the batch's slots so far, at least [`JUDGE`] of them, have nearly all fallen
    /// under summary words of their own.
    #[inline(always)]
    pub(crate) fn wide(&self) -> bool {
        self.debts.taken >= WIDE
    }

    /// Frees slot `slot` of `region`, whose leaf words up to the slot's at least are `leaves`
    /// ([`Release::leaves`]). Aborts if it is free.
    #[inline(always)]
    pub(crate) fn free_slot<H: Host>(
        &mut self,
        release: &mut Release<'_, H>,
        leaves: &[H::W],
        region: RegionId,
        slot: u32,
    ) {
        let word = slot / BITS;
        release.leaf_bit_in(leaves, word, 1 << (slot % BITS));
        self.debts.owe(release, region, word);
    }

    /// [`Direct::free_slot`] for a wide batch: pays the slot's summary word at once
    /// ([`Release::look_leaf`]), the levels' words found in `levels`, and merges what that owes
    /// the level above, which has 64 times fewer words; a debt whose place another takes is
    /// paid by walking on up at once ([`Release::walk_upward`]).
    #[inline(always)]
    pub(crate) fn free_slot_wide<'h, H: Host>(
        &mut self,
        release: &mut Release<'h, H>,
        leaves: &[H::W],
        levels: &Levels<'h, H::W>,
        slot: u32,
    ) {
        let word = slot / BITS;
        release.leaf_bit_in(leaves, word, 1 << (slot % BITS));
        if let Some(up) = release.look_leaf(levels, word) {
            self.debts
                .merge_upward(up, |earlier| release.walk_upward(levels, earlier));
        }
    }

    /// Pays the debts the batch holds by walking up at once, as a wide batch pays its later
    /// slots' ([`Release::walk_up`]): for a batch judged wide, whose debts the table could not
    /// merge. Debts in another region than `levels`' wait for [`Direct::finish`].
    #[inline(never)]
    pub(crate) fn pay_by_walking<'h, H: Host>(
        &mut self,
        release: &mut Release<'h, H>,
        levels: &Levels<'h, H::W>,
    ) {
        self.debts.pay_by_walking(release, levels);
    }

    /// Frees the slots `bits` of leaf word `word` of `region` with one OR. Aborts if one of
    /// them is free.
    pub(crate) fn free<H: Host>(
        &mut self,
        release: &mut Release<'_, H>,
        region: RegionId,
        word: u32,
        bits: u64,
    ) {
        release.leaf_unowed(region, word, bits);
        self.debts.owe(release, region, word);
    }

    /// Owes `release` every summary bit still owed, for its walk up the tree.
    #[inline(never)]
    pub(crate) fn finish<H: Host>(&mut self, release: &mut Release<'_, H>) {
        self.debts.pay_all(release);
    }
}

/// Places in each of [`Debts`]'s tables.
const DEBTS: usize = 64;

/// Places in [`Debts`]'s table of what summary words owe the level above, which has 64 times
/// fewer words than the summary level.
const UPWARD: usize = 32;

/// A key no summary word has: [`Debts`]'s keys are below 2^38.
const NO_KEY: u64 = u64::MAX;

/// What a scattered batch owes the summary words above the leaf words it has freed, and what
/// those owe the words above them in turn, merged per word.
///
/// Two small tables with a place per word, chosen by its low bits: per summary word, a bit for
/// each leaf word under it that the batch freed; and per word of the next level, the bits its
/// summary words owe it. A word whose place another holds pays that one's debt first: more
/// debts than one per word, never fewer. A batch of slots under a few dozen summary words, as
/// a program's freed structure of a few hundred thousand objects is, merges them in the first
/// table and pays each once. A batch spread wider, over more summary words than it has slots,
/// cannot merge them, and pays each summary word as its place is taken; the second table still
/// merges what those owe the level above, which has 64 times fewer words, so the walk up the
/// tree is not made once per slot.
struct Debts {
    /// Per place, the summary word's key (`region << 26 | word / 64`), or [`NO_KEY`]; and the
    /// bits it is owed.
    keys: [u64; DEBTS],
    words: [u64; DEBTS],
    /// Per place, the region the key names, so that paying needs no lookup.
    regions: [Option<RegionId>; DEBTS],
    /// A bit per place that holds a debt.
    held: u64,
    /// Places given to a summary word so far.
    taken: u32,
    /// Per place, what a word of the next level is owed.
    upward: [Option<Upward>; UPWARD],
}

impl Debts {
    #[inline(always)]
    fn new() -> Debts {
        Debts {
            keys: [NO_KEY; DEBTS],
            words: [0; DEBTS],
            regions: [None; DEBTS],
            held: 0,
            taken: 0,
            upward: [None; UPWARD],
        }
    }

    /// Owes the summary bit of leaf word `word` of `region`.
    #[inline(always)]
    fn owe<H: Host>(&mut self, release: &mut Release<'_, H>, region: RegionId, word: u32) {
        let key = (region.index() as u64) << 26 | (word / BITS) as u64;
        let at = key as usize % DEBTS;
        if self.keys[at] != key {
            self.hold(release, at, key, region);
        }
        self.words[at] |= 1 << (word % BITS);
    }

    /// Gives place `at` to summary word `key`, paying the debt of the one that held it.
    #[inline(never)]
    fn hold<H: Host>(
        &mut self,
        release: &mut Release<'_, H>,
        at: usize,
        key: u64,
        region: RegionId,
    ) {
        let at = at % DEBTS;
        if self.keys[at] != NO_KEY {
            self.pay(release, at);
        }
        self.keys[at] = key;
        self.regions[at] = Some(region);
        self.held |= 1 << at;
        self.taken += 1;
    }

    /// Pays the debts of `levels`' region by walking up ([`Release::walk_up`]).
    fn pay_by_walking<'h, H: Host>(
        &mut self,
        release: &mut Release<'h, H>,
        levels: &Levels<'h, H::W>,
    ) {
        let mut held = self.held;
        while held != 0 {
            let at = held.trailing_zeros() as usize;
            held &= held - 1;
            if self.regions[at] != Some(levels.region()) {
                continue;
            }
            let key = core::mem::replace(&mut self.keys[at], NO_KEY);
            let words = core::mem::replace(&mut self.words[at], 0);
            self.held &= !(1 << at);
            release.walk_up(levels, (key as u32 & ((1 << 26) - 1)) * BITS, words);
        }
    }

    /// Pays every debt: the summary words', then what they owe above, which goes to `release`.
    fn pay_all<H: Host>(&mut self, release: &mut Release<'_, H>) {
        let mut held = self.held;
        while held != 0 {
            let at = held.trailing_zeros() as usize;
            held &= held - 1;
            self.pay(release, at);
        }
        for debt in self.upward.iter_mut().filter_map(Option::take) {
            release.owe_upward(debt);
        }
    }

    /// Pays the debt at place `at`, which holds one, and empties the place: sets the summary
    /// word's bits, reading it first (`Look` in `tla/Summary.tla`), and owes what it owes above.
    #[inline(always)]
    fn pay<H: Host>(&mut self, release: &mut Release<'_, H>, at: usize) {
        let at = at % DEBTS;
        let key = core::mem::replace(&mut self.keys[at], NO_KEY);
        let words = core::mem::replace(&mut self.words[at], 0);
        self.held &= !(1 << at);
        let Some(region) = self.regions[at] else {
            release.abort("a summary debt with no region");
        };
        let first = (key as u32 & ((1 << 26) - 1)) * BITS;
        if let Some(debt) = release.pay_leaves(region, first, words, false) {
            self.owe_upward(release, debt);
        }
    }

    /// Merges `debt` with what its word is owed, paying the debt of a word whose place it takes.
    fn owe_upward<H: Host>(&mut self, release: &mut Release<'_, H>, debt: Upward) {
        self.merge_upward(debt, |earlier| release.owe_upward(earlier));
    }

    /// Merges `debt` with what its word is owed, handing the debt of a word whose place it
    /// takes to `pay`.
    #[inline(always)]
    fn merge_upward(&mut self, debt: Upward, pay: impl FnOnce(Upward)) {
        let place = &mut self.upward[debt.key as usize % UPWARD];
        match place {
            Some(held) if held.key == debt.key => {
                held.bits |= debt.bits;
                held.must |= debt.must;
            }
            _ => {
                if let Some(earlier) = place.replace(debt) {
                    pay(earlier);
                }
            }
        }
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
