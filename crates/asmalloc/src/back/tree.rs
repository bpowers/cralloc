//! The free bitmaps and their summary bits, as one tree per class.
//!
//! Each region's leaf bitmap has one bit per slot, set when the slot is free. Above it, each
//! summary level has one bit per word of the level below, set when "something below may be
//! free", up to a single top word per region. The tree continues above the regions: a class has
//! one word per 64 regions with a bit per region, set when the region's top word may be
//! non-zero, and one top word with a bit per such word. So a search walks from the class's top
//! word down to the lowest leaf word that has free slots, which visits the class's regions in
//! address order and each region's words in address order.
//!
//! This is the protocol of `tla/Summary.tla` with `Heal = TRUE`, applied at every level, the
//! class levels included:
//!
//! - **claim**: walk down from the top through the lowest set bit of each word. A word found
//!   empty on the way has a stale summary bit: clear it (below) and start again. At a leaf word,
//!   take bits with one compare-and-swap against the value read; if that fails, start again; if
//!   it took every bit, clear the word's summary bit.
//! - **release**: set the bits with one atomic OR; a bit already set is a double free. Then walk
//!   up: at each level read the summary bit and, if it is clear, set it with an atomic OR. After
//!   an OR that found the summary word empty, the next level's bit is set without reading it
//!   first. The walk always goes to the top (the healing free), so a summary bit left clear by a
//!   thread that vanished in a fork is repaired by the next free below it. A batch of releases
//!   ([`Release`]) shares one walk: each summary word is looked at once for all the bits it
//!   owes.
//! - **clear**: whoever empties a word, or finds one empty under a set bit, clears the bit with
//!   an atomic AND and reads the word again. If the word is no longer empty it sets the bit back
//!   by walking up as a release does. Otherwise, if the AND emptied the summary word, that word
//!   is now one this thread emptied, and it clears that word's bit in turn.
//!
//! The summary bits are hints: they may be set over nothing free, and a word with free bits
//! never has its bit clear once the threads working on it have finished.

use super::layout::{Layout, MAX_LEVELS};
use super::word::{BITS, Word, lowest};
use crate::os::HugePages;
use crate::space::RegionId;

/// What the protocols need from their surroundings: the class's words, its regions' metadata
/// words, and the OS calls on a region. Production code implements it over the heap
/// (`super::host`); Kani harnesses and the interleaving tests implement it over a few words.
pub(crate) trait Host {
    /// The word type.
    type W: Word;

    /// The layout of the class's regions.
    fn layout(&self) -> &Layout;

    /// The region with this index, if it is an object region of the heap.
    fn region(&self, index: usize) -> Option<RegionId>;

    /// Word `word` of a region's metadata. The region is assigned to this class, and the word is
    /// one the caller knows to be committed: in the fixed part, or a leaf word covering a slot
    /// below the watermark.
    fn meta(&self, region: RegionId, word: u32) -> &Self::W;

    /// The class word for regions `64 * index .. 64 * index + 64`.
    fn class_word(&self, index: u32) -> &Self::W;

    /// The class's top word, with a bit per class word.
    fn class_top(&self) -> &Self::W;

    /// Makes `len` bytes at `offset` in a region writable. Both are page multiples.
    fn commit(&self, region: RegionId, offset: usize, len: usize) -> bool;

    /// Makes `len` bytes at `offset` in a region's metadata block writable. Both are page
    /// multiples.
    fn commit_meta(&self, region: RegionId, offset: usize, len: usize) -> bool;

    /// Advises for or against huge pages over part of a region. `false` if the OS refused;
    /// callers that do not care may ignore it, since it is advice.
    fn advise(&self, region: RegionId, offset: usize, len: usize, advice: HugePages) -> bool;

    /// Returns part of a region to the OS; it reads as zero afterwards.
    ///
    /// # Safety
    ///
    /// No thread reads or writes the range during the call, and nothing in it is needed.
    unsafe fn purge(&self, region: RegionId, offset: usize, len: usize) -> bool;

    /// Ends the process: the heap has been misused.
    fn abort(&self, message: &str) -> !;
}

/// A word of the tree, as other modules and the tests name one. The walks here use [`Spot`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Node {
    /// Word `index` of level `level` of a region's tree; level 0 is the leaf bitmap.
    Region {
        region: RegionId,
        level: u8,
        index: u32,
    },
    /// A class word: one bit per region. Only the tests' own walk names one.
    #[cfg(test)]
    Class(u32),
    /// The class's top word.
    #[cfg(test)]
    Top,
}

impl Node {
    /// Leaf word `index` of a region.
    #[inline(always)]
    pub(crate) fn leaf(region: RegionId, index: u32) -> Node {
        Node::Region {
            region,
            level: 0,
            index,
        }
    }
}

/// The word a node names.
#[inline(always)]
pub(crate) fn word<H: Host>(host: &H, node: Node) -> &H::W {
    match node {
        Node::Region {
            region,
            level,
            index,
        } => host.meta(region, host.layout().tree_word(level, index)),
        #[cfg(test)]
        Node::Class(index) => host.class_word(index),
        #[cfg(test)]
        Node::Top => host.class_top(),
    }
}

/// Bits taken from one leaf word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Taken {
    /// The region.
    pub(crate) region: RegionId,
    /// The leaf word.
    pub(crate) word: u32,
    /// The bits taken: slot `64 * word + b` for each set bit `b`.
    pub(crate) bits: u64,
}

/// A word of the tree by number, as the walks use it. Levels `0..=height` are a region's
/// (level 0 its leaf bitmap, `height` its top word); level `height + 1` is the class word that
/// has the region's bit, with `index` its number; level `height + 2` is the class's top word,
/// with `index` zero. Above the region's own levels `region` is any region under the word,
/// which the walk carries along and nothing reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Spot {
    region: RegionId,
    level: u8,
    index: u32,
}

impl Spot {
    #[inline(always)]
    fn leaf(region: RegionId, index: u32) -> Spot {
        Spot {
            region,
            level: 0,
            index,
        }
    }

    /// The spot of a class word, named by its first region, which must exist.
    fn class_word<H: Host>(host: &H, index: u32) -> Spot {
        let Some(region) = host.region((index * BITS) as usize) else {
            host.abort("a class summary bit names a region outside the heap")
        };
        Spot {
            region,
            level: host.layout().height() + 1,
            index,
        }
    }

    /// The word this spot names.
    #[inline(always)]
    fn word<H: Host>(self, host: &H) -> &H::W {
        let layout = host.layout();
        let height = layout.height();
        if self.level <= height {
            host.meta(self.region, layout.tree_word(self.level, self.index))
        } else if self.level == height + 1 {
            host.class_word(self.index)
        } else {
            host.class_top()
        }
    }

    /// The word above this one, and the bit that stands for this one in it.
    #[inline(always)]
    fn parent(self, height: u8) -> Option<(Spot, u64)> {
        let (index, bit) = if self.level < height {
            (self.index / BITS, self.index % BITS)
        } else if self.level == height {
            let r = self.region.index() as u32;
            (r / BITS, r % BITS)
        } else if self.level == height + 1 {
            (0, self.index % BITS)
        } else {
            return None;
        };
        let above = Spot {
            region: self.region,
            level: self.level + 1,
            index,
        };
        Some((above, 1 << bit))
    }

    /// A number that tells apart the words of one level: the region and the index in a
    /// region's levels, the index alone above them.
    #[inline(always)]
    fn key(self, height: u8) -> u64 {
        if self.level <= height {
            (self.region.index() as u64) << 32 | self.index as u64
        } else {
            self.index as u64
        }
    }

    /// The spot at `level` with this key. Above a region's levels the key has no region, and
    /// `under` stands in for it.
    #[inline(always)]
    fn of_key<H: Host>(host: &H, level: u8, key: u64, under: RegionId) -> Spot {
        let region = if level <= host.layout().height() {
            match host.region((key >> 32) as usize) {
                Some(region) => region,
                None => host.abort("a summary word of a region outside the heap"),
            }
        } else {
            under
        };
        Spot {
            region,
            level,
            index: key as u32,
        }
    }
}

/// Claims up to `n` free slots, at least one, from the lowest leaf word of the class that has
/// any, or returns `None` if the class has none free. `n` must not be zero.
pub(crate) fn claim<H: Host>(host: &H, n: u32) -> Option<Taken> {
    let height = host.layout().height();
    loop {
        let top = host.class_top().load();
        if top == 0 {
            return None;
        }
        let class_word = top.trailing_zeros();
        let regions = host.class_word(class_word).load();
        if regions == 0 {
            clear(host, Spot::class_word(host, class_word));
            continue;
        }
        let Some(region) = host.region((class_word * BITS + regions.trailing_zeros()) as usize)
        else {
            host.abort("a class summary bit names a region outside the heap")
        };
        let top = Spot {
            region,
            level: height,
            index: 0,
        };
        if let Some(taken) = descend(host, top, n) {
            return Some(taken);
        }
    }
}

/// Walks down from `spot` through the lowest set bit of each word and claims up to `n` slots
/// from the leaf word it reaches. `None` if a word on the way is empty (its stale summary bit
/// is cleared) or the compare-and-swap fails: the caller searches again.
#[inline]
fn descend<H: Host>(host: &H, mut spot: Spot, n: u32) -> Option<Taken> {
    loop {
        let word = spot.word(host);
        let seen = word.load();
        if seen == 0 {
            clear(host, spot);
            return None;
        }
        if spot.level == 0 {
            let bits = lowest(seen, n);
            word.compare_exchange(seen, seen & !bits).ok()?;
            if bits == seen {
                clear(host, spot);
            }
            return Some(Taken {
                region: spot.region,
                word: spot.index,
                bits,
            });
        }
        spot = Spot {
            level: spot.level - 1,
            index: spot.index * BITS + seen.trailing_zeros(),
            ..spot
        };
    }
}

/// Where a run of claims has got to: the leaf word it claimed from last. Only ever made by
/// [`claim_next`] from its own claim, and kept for no longer than one fill.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Cursor {
    region: RegionId,
    word: u32,
}

/// [`claim`] for one of a run of claims, as a fill makes them. With a `cursor` from the run's
/// last claim, the search starts there: that word if it has free slots left, else the lowest
/// word above it in its region that the summary bits lead to, found by walking up only as far
/// as a summary word with a set bit beyond the path. Only when that leads nowhere, or meets a
/// stale bit or a lost race, does it search from the class's top word. Updates `cursor`.
///
/// The last claim took the lowest free slots there were, so the slots below the cursor were
/// not free a moment ago, and the run still takes the lowest it can see. What it can miss is a
/// slot freed below the cursor since, by another thread: the order of the two is a race either
/// way, and the next fill starts from the top. Words found empty on the way are cleared as a
/// claim from the top clears them, and the claim itself is the same compare-and-swap.
pub(crate) fn claim_next<H: Host>(host: &H, n: u32, cursor: &mut Option<Cursor>) -> Option<Taken> {
    let taken = match cursor.and_then(|at| claim_after(host, n, at)) {
        Some(taken) => taken,
        None => claim(host, n)?,
    };
    *cursor = Some(Cursor {
        region: taken.region,
        word: taken.word,
    });
    Some(taken)
}

/// The search of [`claim_next`] within the cursor's region: `None` if it leads nowhere there.
fn claim_after<H: Host>(host: &H, n: u32, at: Cursor) -> Option<Taken> {
    let height = host.layout().height();
    let leaf = Spot::leaf(at.region, at.word);
    if leaf.word(host).load() != 0 {
        return descend(host, leaf, n);
    }
    let mut spot = leaf;
    while spot.level < height {
        let (index, bit) = (spot.index / BITS, spot.index % BITS);
        let above = Spot {
            level: spot.level + 1,
            index,
            ..spot
        };
        let beyond = above.word(host).load() & (u64::MAX << bit << 1);
        if beyond != 0 {
            let next = Spot {
                index: index * BITS + beyond.trailing_zeros(),
                ..spot
            };
            return descend(host, next, n);
        }
        spot = above;
    }
    None
}

/// Claims exactly `bits` of a leaf word if every one of them is free, as a purge does. Returns
/// whether it did.
pub(crate) fn claim_exact<H: Host>(host: &H, region: RegionId, index: u32, bits: u64) -> bool {
    let spot = Spot::leaf(region, index);
    let leaf = spot.word(host);
    let mut seen = leaf.load();
    loop {
        if seen & bits != bits {
            return false;
        }
        match leaf.compare_exchange(seen, seen & !bits) {
            Ok(_) => {
                if seen == bits {
                    clear(host, spot);
                }
                return true;
            }
            Err(now) => seen = now,
        }
    }
}

/// Frees `bits` of a leaf word. Aborts if any of them is already free: that is a double free.
pub(crate) fn release<H: Host>(host: &H, region: RegionId, index: u32, bits: u64) {
    let mut release = Release::new(host, region);
    release.leaf(region, index, bits);
    release.finish();
}

/// Summary levels a walk up can owe a bit in: a region's levels above its leaf bitmap (at most
/// `MAX_LEVELS - 1`) and the two class levels.
const OWED_LEVELS: usize = MAX_LEVELS + 1;

/// Summary bits a release has yet to make sure of: every bit of `bits` in the word whose
/// [`Spot::key`] is `key`, each standing for a word below that is not empty; nothing if `bits`
/// is zero. With `must` non-zero, at least one of those words was empty before this thread's
/// write, so the bits are set without a look first. Every field is a whole word: the entries
/// are written and read back at once, and narrower fields would keep the processor from
/// forwarding the stores to the loads.
#[derive(Clone, Copy, Debug)]
struct Owed {
    key: u64,
    bits: u64,
    must: u64,
}

impl Owed {
    const NOTHING: Owed = Owed {
        key: 0,
        bits: 0,
        must: 0,
    };
}

/// Frees of any number of leaf words, with one walk up the tree for all of them.
///
/// Each [`Release::leaf`] sets its bits with one atomic OR, as a single release does, and owes
/// the summary bit above that word. A debt is paid when a bit for another word of the same
/// level comes along, and at [`Release::finish`]. Paying is what a single release's walk does
/// at that level (a look, and an atomic OR of whatever is missing; or the OR alone after a word
/// was found empty), done for all the bits owed in that word at once, and it owes the bit
/// above in turn, up to the class's top word. So words that share summary words, as neighbours
/// do, share one walk, and every summary bit above every word freed has been seen set when
/// `finish` returns.
///
/// Until then a word freed early in the batch may sit under a clear summary bit. That is the
/// window a single release has between its OR and its walk, made longer, and
/// `tla/Summary.tla` allows it: a thread may owe a bit for as long as it likes, and a fill
/// that cannot see a slot yet takes another. Setting several bits of one word with one OR is
/// the schedule of the model in which the releases of those words make their ORs back to back.
///
/// A `Release` dropped without `finish` leaves its debts unpaid, as a thread that vanishes in a
/// fork does; the next free under each word heals it.
pub(crate) struct Release<'h, H: Host> {
    host: &'h H,
    height: u8,
    /// A region of the class, for the spots of class words (see [`Spot::of_key`]).
    under: RegionId,
    /// What is owed at each summary level, at the level's number minus one.
    owed: [Owed; OWED_LEVELS],
}

impl<'h, H: Host> Release<'h, H> {
    /// A batch with nothing freed yet, for frees into `region` and any other region of the
    /// class.
    #[inline]
    pub(crate) fn new(host: &'h H, region: RegionId) -> Release<'h, H> {
        Release {
            host,
            height: host.layout().height(),
            under: region,
            owed: [Owed::NOTHING; OWED_LEVELS],
        }
    }

    /// Frees `bits` of a leaf word with one atomic OR. Aborts if any of them is already free:
    /// that is a double free.
    #[inline]
    pub(crate) fn leaf(&mut self, region: RegionId, index: u32, bits: u64) {
        let spot = Spot::leaf(region, index);
        let old = spot.word(self.host).fetch_or(bits);
        if old & bits != 0 {
            self.host
                .abort("double free: an object was freed while already free");
        }
        self.owe(spot, old == 0);
    }

    /// [`Release::leaf`] for a region given by its number.
    #[inline]
    pub(crate) fn leaf_by_number(&mut self, region: usize, index: u32, bits: u64) {
        let Some(region) = self.host.region(region) else {
            self.abort("a free into a region outside the heap");
        };
        self.leaf(region, index, bits);
    }

    /// Ends the process: the heap has been misused.
    pub(crate) fn abort(&self, message: &str) -> ! {
        self.host.abort(message)
    }

    /// Makes sure of every summary bit still owed, from the lowest level up.
    pub(crate) fn finish(mut self) {
        for at in 0..OWED_LEVELS {
            let owed = core::mem::replace(&mut self.owed[at], Owed::NOTHING);
            if owed.bits != 0 {
                self.pay(at, owed);
            }
        }
    }

    /// Owes the bit above `spot`, whose word is not empty; `must` if it was empty before this
    /// thread's write.
    #[inline]
    fn owe(&mut self, spot: Spot, must: bool) {
        let Some((above, bit)) = spot.parent(self.height) else {
            return;
        };
        let key = above.key(self.height);
        let at = above.level as usize - 1;
        let Some(entry) = self.owed.get_mut(at) else {
            self.host.abort("a summary level above the top of the tree");
        };
        if entry.bits != 0 && entry.key == key {
            entry.bits |= bit;
            entry.must |= must as u64;
            return;
        }
        let earlier = core::mem::replace(
            entry,
            Owed {
                key,
                bits: bit,
                must: must as u64,
            },
        );
        if earlier.bits != 0 {
            self.pay(at, earlier);
        }
    }

    /// Sets whatever of `owed`, at index `at` of the levels, is clear, and owes the bit above
    /// it in turn: the healing walk always goes to the top.
    fn pay(&mut self, at: usize, owed: Owed) {
        let spot = Spot::of_key(self.host, at as u8 + 1, owed.key, self.under);
        let word = spot.word(self.host);
        let was_empty = if owed.must != 0 {
            word.fetch_or(owed.bits) == 0
        } else {
            let missing = owed.bits & !word.load();
            missing != 0 && word.fetch_or(missing) == 0
        };
        self.owe(spot, was_empty);
    }
}

/// Walks up from `spot`, whose word is not empty, making sure every summary bit on the way to
/// the top is set. `must` means the bit above `spot` is to be set without reading it first,
/// because the word was empty before this thread's write.
fn up<H: Host>(host: &H, spot: Spot, must: bool) {
    let mut release = Release::new(host, spot.region);
    release.owe(spot, must);
    release.finish();
}

/// `spot`'s word was seen empty by this thread, which emptied it or found it empty under a set
/// bit: clear the bit above it and read the word again.
fn clear<H: Host>(host: &H, mut spot: Spot) {
    let height = host.layout().height();
    while let Some((above, bit)) = spot.parent(height) {
        let left = above.word(host).fetch_and(!bit) & !bit;
        if spot.word(host).load() != 0 {
            up(host, spot, true);
            return;
        }
        if left != 0 {
            return;
        }
        spot = above;
    }
}
