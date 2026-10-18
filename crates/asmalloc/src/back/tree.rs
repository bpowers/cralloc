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
//!   take bits with one compare-and-swap against the value read; if that fails, try again
//!   against the value it found; if it took every bit, clear the word's summary bit.
//! - **a claim that keeps the bits** ([`claim`] with `keep`, which every large class's
//!   allocation is): as a claim, except that when it takes every bit of a word it does not
//!   clear the word's summary bit. In the model that is a thread that holds its clear for as long
//!   as it likes, as a thread lost in a fork holds it for ever, so `Partition` and `Visible`
//!   hold as before; the bit is a hint over nothing free, and the next search that finds the
//!   word empty clears it, as it clears any stale bit. The point is a class whose slots are taken
//!   and freed one at a time: a claim that clears walks up clearing to the top, and the free
//!   that follows walks up setting the same bits again. With the bits kept, the free finds them
//!   set: it found the leaf word empty, so it still makes one OR of the bit above it without
//!   looking, and writes nothing above that.
//! - **a run of claims** (a fill, [`claim_next`]) is the model's `Run`: it continues from where
//!   its last claim was rather than from the top, moves past a group of 64 words where it meets
//!   another claimer, leaves a word it finds empty to whoever emptied it, and clears the bits of
//!   the words it empties in one group with one atomic AND and a re-read of each.
//! - **release**: set the bits with one atomic OR; a bit already set is a double free. Then walk
//!   up: at each level read the summary bit and, if it is clear, set it with an atomic OR. After
//!   an OR that found the summary word empty, the next level's bit is set without reading it
//!   first. The walk always goes to the top (the healing free), so a summary bit left clear by a
//!   thread that vanished in a fork is repaired by the next free below it. A batch of releases
//!   ([`Release`]) shares one walk: each summary word is looked at once for all the bits it
//!   owes. The release of one slot ([`release_one`]) is the same steps for one bit with no
//!   batch's bookkeeping.
//! - **clear**: whoever empties a word, or finds one empty under a set bit, clears the bit with
//!   an atomic AND and reads the word again. If the word is no longer empty it sets the bit back
//!   by walking up as a release does. Otherwise, if the AND emptied the summary word, that word
//!   is now one this thread emptied, and it clears that word's bit in turn.
//!
//! The summary bits are hints: they may be set over nothing free, and a word with free bits
//! never has its bit clear once the threads working on it have finished.
//!
//! Until they have, a free slot can be invisible, and batching a run's clears widens two such
//! windows. A free that completes while a run holds the clear of its word is hidden from the
//! run's AND until the run reads that word again, so a search in that window may find nothing
//! and carve though the slot is free. And a run lost in a fork between its AND and its re-reads
//! leaves every word of its batch that was refilled meanwhile invisible, not one word, until a
//! free lands in each.

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
///
/// With `keep`, a claim that empties its word leaves the summary bits above it set: the
/// model's clear, held for as long as the thread likes, as a thread lost in a fork holds it.
/// The next search that finds the word empty clears them, as it clears any stale bit. A class
/// whose slots are freed and taken one at a time, as large ones are, would otherwise clear the
/// bits up to the top on every claim and set them again on every free.
pub(crate) fn claim<H: Host>(host: &H, n: u32, keep: bool) -> Option<Taken> {
    loop {
        let leaf = from_top(host)?;
        match take(host, leaf, n) {
            Take::Took { bits, emptied, .. } => {
                if emptied && !keep {
                    clear(host, leaf);
                }
                return Some(Taken {
                    region: leaf.region,
                    word: leaf.index,
                    bits,
                });
            }
            Take::Empty { raced } => {
                if !raced {
                    clear(host, leaf);
                }
            }
        }
    }
}

/// The leaf word a search from the class's top word leads to, through the lowest set bit of
/// each word, or `None` if the class has nothing free. A summary word found empty on the way
/// has a stale bit: it is cleared, and the search starts again.
fn from_top<H: Host>(host: &H) -> Option<Spot> {
    let height = host.layout().height();
    'search: loop {
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
        let mut spot = Spot {
            region,
            level: height,
            index: 0,
        };
        while spot.level > 0 {
            let seen = spot.word(host).load();
            if seen == 0 {
                clear(host, spot);
                continue 'search;
            }
            spot = Spot {
                level: spot.level - 1,
                index: spot.index * BITS + seen.trailing_zeros(),
                ..spot
            };
        }
        return Some(spot);
    }
}

/// What [`take`] found at a leaf word.
#[derive(Clone, Copy, Debug)]
enum Take {
    /// It took `bits`. `emptied`: they were all the word had, so the word's summary bit is
    /// this thread's to clear. `raced`: another thread took bits of the word between this
    /// thread's look and its claim.
    Took {
        bits: u64,
        emptied: bool,
        raced: bool,
    },
    /// The word was empty. `raced`: it was not when this thread first looked, so another
    /// thread emptied it and will clear its summary bit.
    Empty { raced: bool },
}

/// Takes the lowest `n` free bits of a leaf word, or as many as it has, with one
/// compare-and-swap. A lost compare-and-swap does not end the attempt: it tries again
/// against the value it found, so it fails only when the word is empty.
///
/// Each attempt is the model's claim, a look at the word and then a compare-and-swap against
/// what was seen; the one that succeeds is a claim like any other.
#[inline]
fn take<H: Host>(host: &H, leaf: Spot, n: u32) -> Take {
    let word = leaf.word(host);
    let mut seen = word.load();
    let mut raced = false;
    loop {
        if seen == 0 {
            return Take::Empty { raced };
        }
        let bits = lowest(seen, n);
        match word.compare_exchange(seen, seen & !bits) {
            Ok(_) => {
                return Take::Took {
                    bits,
                    emptied: bits == seen,
                    raced,
                };
            }
            // Bits that went are another claimer's doing; bits that came are a free's.
            Err(now) => {
                raced |= seen & !now != 0;
                seen = now;
            }
        }
    }
}

/// Where a run of claims has got to, and the summary bits it owes. Only ever made by
/// [`claim_next`], and kept for no longer than one fill, which ends it with [`finish_run`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Cursor {
    region: RegionId,
    /// The leaf word the next claim of the run looks at first.
    next: u32,
    /// The group the run is in: its level-1 summary word, which has a bit for each of 64 leaf
    /// words.
    group: u32,
    /// The words of the group the run has emptied and whose summary bits it has not yet
    /// cleared, as bits of the group's summary word.
    emptied: u64,
}

impl Cursor {
    fn at(leaf: Spot) -> Cursor {
        Cursor {
            region: leaf.region,
            next: leaf.index,
            group: leaf.index / BITS,
            emptied: 0,
        }
    }
}

/// [`claim`] for one of a run of claims, as a fill makes them. Updates `cursor`, which starts
/// as `None`; a fill ends the run with [`finish_run`].
///
/// The first claim searches from the class's top word. Later ones continue from the cursor in
/// its region: the word claimed from last if it has slots left, else the lowest word beyond it
/// that the summary bits lead to, found by walking up only as far as a summary word with a set
/// bit beyond the path. Only when the region has nothing more does the run search from the top
/// again, so it returns `None` only when a search from the top finds nothing.
///
/// Fillers of one class all start at the lowest free word, and if they went on side by side
/// they would meet at every word: each claim a contended compare-and-swap, each emptied word a
/// contended clear of the summary word they share. So a run that meets another thread there
/// (another claimer took bits of the word under it, or a word it was led to is empty) moves to
/// the next group of 64 words, whose leaf words are cache lines of their own. After a few
/// meetings each filler works alone, near the bottom of the heap still.
///
/// A run does not clear the summary bit of a word it finds empty: another thread emptied it and
/// clears it, unless that thread was lost in a fork, and then the next search from the top
/// clears it. And the bits of the words the run empties itself are cleared together, one
/// atomic AND for each group the run leaves (`clear_group`), not one for each word.
///
/// The run takes the lowest slots it can see, except where it moved past another thread. What
/// it can miss is a slot freed behind it since, by another thread, which is a race either way,
/// and the next fill starts from the top.
///
/// Out of line: inlined into `Back::fill`, its state took registers from the fill's loop over
/// the slots, which then reloaded two values from the stack for every slot.
#[inline(never)]
pub(crate) fn claim_next<H: Host>(host: &H, n: u32, cursor: &mut Option<Cursor>) -> Option<Taken> {
    let mut run = *cursor;
    loop {
        let found = run.and_then(|at| seek(host, at.region, at.next));
        let (leaf, at) = match (found, run) {
            (Some(index), Some(at)) => {
                let leaf = Spot::leaf(at.region, index);
                if leaf.index / BITS == at.group {
                    (leaf, at)
                } else {
                    clear_group(host, at);
                    (leaf, Cursor::at(leaf))
                }
            }
            _ => {
                if let Some(at) = run.take() {
                    clear_group(host, at);
                }
                *cursor = None;
                let leaf = from_top(host)?;
                (leaf, Cursor::at(leaf))
            }
        };
        let from_top = run.is_none();
        let (bits, emptied, raced) = match take(host, leaf, n) {
            Take::Took {
                bits,
                emptied,
                raced,
            } => (bits, emptied, raced),
            Take::Empty { raced } => {
                // A search from the top that finds a word empty under its bit clears the bit,
                // which is what removes a stale one; a run leaves it to the thread that
                // emptied the word.
                if from_top && !raced {
                    clear(host, leaf);
                }
                run = Some(Cursor {
                    next: past_group(leaf.index),
                    ..at
                });
                continue;
            }
        };
        let mut at = at;
        if emptied {
            if host.layout().height() == 0 {
                // The word's summary bit is a class word's, shared with other regions.
                clear(host, leaf);
            } else {
                at.emptied |= 1 << (leaf.index % BITS);
            }
        }
        at.next = if raced {
            past_group(leaf.index)
        } else if emptied {
            leaf.index + 1
        } else {
            leaf.index
        };
        *cursor = Some(at);
        return Some(Taken {
            region: leaf.region,
            word: leaf.index,
            bits,
        });
    }
}

/// Ends a run of claims: clears the summary bits it owes. A run that is never finished, as in
/// a thread lost in a fork, leaves them set over empty words, which the protocol allows.
pub(crate) fn finish_run<H: Host>(host: &H, cursor: Option<Cursor>) {
    if let Some(at) = cursor {
        clear_group(host, at);
    }
}

/// The first leaf word of the group after the one `index` is in.
#[inline(always)]
pub(super) fn past_group(index: u32) -> u32 {
    (index | (BITS - 1)) + 1
}

/// The lowest leaf word of `region` at or after word `first` that the summary bits lead to,
/// or `None` if they lead to none. It reads and writes nothing else: a summary word found empty
/// on the way down is passed over, as its stale bit is another thread's to clear.
fn seek<H: Host>(host: &H, region: RegionId, first: u32) -> Option<u32> {
    let layout = host.layout();
    let height = layout.height();
    // Looking for the lowest word of `level` at or after `index` whose bit is set.
    let (mut level, mut index) = (0u8, first);
    loop {
        // Up, to the lowest summary word with a set bit at or after the path.
        loop {
            if level == height {
                if index > 0 {
                    return None;
                }
                break;
            }
            if index >= layout.level_words(level) {
                level += 1;
                index = layout.level_words(level);
                continue;
            }
            let above = Spot {
                region,
                level: level + 1,
                index: index / BITS,
            };
            let set = above.word(host).load() & (u64::MAX << (index % BITS));
            if set != 0 {
                index = above.index * BITS + set.trailing_zeros();
                break;
            }
            level += 1;
            index = above.index + 1;
        }
        // Down, through the lowest set bit of each word.
        loop {
            if level == 0 {
                return Some(index);
            }
            let seen = Spot {
                region,
                level,
                index,
            }
            .word(host)
            .load();
            if seen == 0 {
                index += 1;
                break;
            }
            level -= 1;
            index = index * BITS + seen.trailing_zeros();
        }
    }
}

/// Clears the summary bits of the words a run emptied in its group, with one atomic AND of the
/// group's summary word, then reads each of those words again, as a clear of one word does.
/// A word that is not empty any more has its bit set back by a walk up, as a release makes one.
/// If the AND emptied the summary word and every word read empty, the summary word is now one
/// this thread emptied, and its own bit is cleared in turn.
///
/// This is the clears of the model made back to back, one for each word, which the model
/// allows: a thread may hold an emptied word's clear for as long as it likes, and these clears
/// are of distinct bits of one word, so one AND makes them all at one instant.
fn clear_group<H: Host>(host: &H, at: Cursor) {
    if at.emptied == 0 {
        return;
    }
    let group = Spot {
        region: at.region,
        level: 1,
        index: at.group,
    };
    let left = group.word(host).fetch_and(!at.emptied) & !at.emptied;
    let mut all_empty = true;
    let mut rest = at.emptied;
    while rest != 0 {
        let leaf = Spot::leaf(at.region, at.group * BITS + rest.trailing_zeros());
        rest &= rest - 1;
        if leaf.word(host).load() != 0 {
            all_empty = false;
            up(host, leaf, true);
        }
    }
    if left == 0 && all_empty {
        clear(host, group);
    }
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

/// Frees the one slot `bit` of a leaf word: the model's release for one bit, with none of a
/// batch's bookkeeping. One atomic OR, which aborts if the bit was set already (a double free),
/// then the healing walk to the top: at each level, the OR alone if the word below was empty
/// before this thread's write, else a look and an OR only if the bit is clear.
pub(crate) fn release_one<H: Host>(host: &H, region: RegionId, index: u32, bit: u64) {
    let mut spot = Spot::leaf(region, index);
    let old = spot.word(host).fetch_or(bit);
    if old & bit != 0 {
        host.abort("double free: an object was freed while already free");
    }
    let mut must = old == 0;
    let height = host.layout().height();
    while let Some((above, bit)) = spot.parent(height) {
        let word = above.word(host);
        must = if must || word.load() & bit == 0 {
            word.fetch_or(bit) == 0
        } else {
            false
        };
        spot = above;
    }
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

    /// The region with this number, or an abort if there is none.
    #[inline]
    pub(crate) fn region(&self, number: usize) -> RegionId {
        match self.host.region(number) {
            Some(region) => region,
            None => self.abort("a free into a region outside the heap"),
        }
    }

    /// [`Release::leaf`] without the debt: frees `bits` of leaf word `index` with one atomic OR
    /// and returns whether the word was empty before, leaving the summary bit above it to a
    /// call of [`Release::owe_leaves`] for it and its neighbours. Aborts on a double free.
    #[inline(always)]
    pub(crate) fn leaf_unowed(&mut self, region: RegionId, index: u32, bits: u64) -> bool {
        // The OR's own old value, so that the double-free check is atomic with it. A plain load,
        // the check, and an OR whose result is unused (one `lock or`) measured a cycle cheaper
        // a scattered slot out of fifty, not worth a check that two threads freeing one slot
        // at once could both pass.
        let old = Spot::leaf(region, index).word(self.host).fetch_or(bits);
        if old & bits != 0 {
            self.host
                .abort("double free: an object was freed while already free");
        }
        old == 0
    }

    /// Frees the one slot `bit` of leaf word `index` with one atomic OR, and aborts if it was
    /// free already: a double free. Unlike [`Release::leaf_unowed`] it does not say whether the
    /// word was empty, so the caller owes the summary bit with `must` false ([`owe_leaves`]):
    /// it reads the bit before it sets it, as a free into a word that was not empty does. That
    /// is `Look` in `tla/Summary.tla`. Using only the OR's own bit lets it compile to one
    /// `lock bts`.
    ///
    /// [`owe_leaves`]: Release::owe_leaves
    #[inline(always)]
    pub(crate) fn leaf_bit(&mut self, region: RegionId, index: u32, bit: u64) {
        if Spot::leaf(region, index).word(self.host).fetch_or(bit) & bit != 0 {
            self.host
                .abort("double free: an object was freed while already free");
        }
    }

    /// Owes the summary bits above leaf words of `region` that share one summary word, freed
    /// by [`Release::leaf_unowed`]: `words` has bit `w % 64` for each such word `w`, `index` is
    /// any of them, and `must` says whether any was empty before its OR. One debt for all of
    /// them, which is what [`Release::leaf`] would have owed for each, merged.
    #[inline]
    pub(crate) fn owe_leaves(&mut self, region: RegionId, index: u32, words: u64, must: bool) {
        let Some((above, bit)) = Spot::leaf(region, index).parent(self.height) else {
            return;
        };
        // A region with one leaf word has its bit in a class word, whatever the word's number.
        let bits = if self.height == 0 { bit } else { words };
        self.owe_bits(above, bits, must);
    }

    /// [`Release::owe_leaves`], with the debt paid at once rather than held: for a caller
    /// that has already merged the debts of the words under each summary word, so that
    /// holding them would only replace one with the next.
    #[inline(never)]
    pub(crate) fn pay_leaves(&mut self, region: RegionId, index: u32, words: u64, must: bool) {
        let Some((above, bit)) = Spot::leaf(region, index).parent(self.height) else {
            return;
        };
        let bits = if self.height == 0 { bit } else { words };
        self.settle(above, bits, must);
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
        self.owe_bits(above, bit, must);
    }

    /// Owes `bits` of the summary word `above`, for words below it that are not empty; `must`
    /// if one of them was empty before this thread's write.
    #[inline]
    fn owe_bits(&mut self, above: Spot, bits: u64, must: bool) {
        let key = above.key(self.height);
        let at = above.level as usize - 1;
        let Some(entry) = self.owed.get_mut(at) else {
            self.host.abort("a summary level above the top of the tree");
        };
        if entry.bits != 0 && entry.key == key {
            entry.bits |= bits;
            entry.must |= must as u64;
            return;
        }
        let earlier = core::mem::replace(
            entry,
            Owed {
                key,
                bits,
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
        self.settle(spot, owed.bits, owed.must != 0);
    }

    /// Sets whatever of `bits` in `spot`'s word is clear (all of them unread, with `must`),
    /// and owes the bit above it in turn.
    #[inline(always)]
    fn settle(&mut self, spot: Spot, bits: u64, must: bool) {
        let word = spot.word(self.host);
        let was_empty = if must {
            word.fetch_or(bits) == 0
        } else {
            let missing = bits & !word.load();
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
