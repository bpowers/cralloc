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
//!   thread that vanished in a fork is repaired by the next free below it.
//! - **clear**: whoever empties a word, or finds one empty under a set bit, clears the bit with
//!   an atomic AND and reads the word again. If the word is no longer empty it sets the bit back
//!   by walking up as a release does. Otherwise, if the AND emptied the summary word, that word
//!   is now one this thread emptied, and it clears that word's bit in turn.
//!
//! The summary bits are hints: they may be set over nothing free, and a word with free bits
//! never has its bit clear once the threads working on it have finished.

use super::layout::Layout;
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

    /// Advises for or against huge pages over part of a region. Failure is ignored: it is
    /// advice.
    fn advise(&self, region: RegionId, offset: usize, len: usize, advice: HugePages);

    /// Returns part of a region to the OS; it reads as zero afterwards.
    ///
    /// # Safety
    ///
    /// No thread reads or writes the range during the call, and nothing in it is needed.
    unsafe fn purge(&self, region: RegionId, offset: usize, len: usize) -> bool;

    /// Ends the process: the heap has been misused.
    fn abort(&self, message: &str) -> !;
}

/// A word of the tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Node {
    /// Word `index` of level `level` of a region's tree; level 0 is the leaf bitmap.
    Region {
        region: RegionId,
        level: u8,
        index: u32,
    },
    /// A class word: one bit per region.
    Class(u32),
    /// The class's top word.
    Top,
}

impl Node {
    /// A region's top word.
    #[inline(always)]
    pub(crate) fn region_top(layout: &Layout, region: RegionId) -> Node {
        Node::Region {
            region,
            level: layout.height(),
            index: 0,
        }
    }

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
        Node::Class(index) => host.class_word(index),
        Node::Top => host.class_top(),
    }
}

/// The node above `node` and the bit that stands for `node` in it.
#[inline(always)]
fn parent(layout: &Layout, node: Node) -> Option<(Node, u64)> {
    match node {
        Node::Region {
            region,
            level,
            index,
        } => {
            if level < layout.height() {
                let up = Node::Region {
                    region,
                    level: level + 1,
                    index: index / BITS,
                };
                Some((up, 1 << (index % BITS)))
            } else {
                let r = region.index() as u32;
                Some((Node::Class(r / BITS), 1 << (r % BITS)))
            }
        }
        Node::Class(index) => Some((Node::Top, 1 << (index % BITS))),
        Node::Top => None,
    }
}

/// The node below `node` that bit `bit` of it stands for.
#[inline(always)]
fn child<H: Host>(host: &H, node: Node, bit: u32) -> Node {
    match node {
        Node::Top => Node::Class(bit),
        Node::Class(index) => {
            let Some(region) = host.region((index * BITS + bit) as usize) else {
                host.abort("a class summary bit names a region outside the heap")
            };
            Node::region_top(host.layout(), region)
        }
        Node::Region {
            region,
            level,
            index,
        } => Node::Region {
            region,
            level: level.saturating_sub(1),
            index: index * BITS + bit,
        },
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

/// Claims up to `n` free slots, at least one, from the lowest leaf word of the class that has
/// any, or returns `None` if the class has none free. `n` must not be zero.
pub(crate) fn claim<H: Host>(host: &H, n: u32) -> Option<Taken> {
    'search: loop {
        let mut node = Node::Top;
        loop {
            let seen = word(host, node).load();
            if seen == 0 {
                if node == Node::Top {
                    return None;
                }
                clear(host, node);
                continue 'search;
            }
            if let Node::Region {
                region,
                level: 0,
                index,
            } = node
            {
                let bits = lowest(seen, n);
                match word(host, node).compare_exchange(seen, seen & !bits) {
                    Ok(_) => {
                        if bits == seen {
                            clear(host, node);
                        }
                        return Some(Taken {
                            region,
                            word: index,
                            bits,
                        });
                    }
                    Err(_) => continue 'search,
                }
            }
            node = child(host, node, seen.trailing_zeros());
        }
    }
}

/// Claims exactly `bits` of a leaf word if every one of them is free, as a purge does. Returns
/// whether it did.
pub(crate) fn claim_exact<H: Host>(host: &H, region: RegionId, index: u32, bits: u64) -> bool {
    let node = Node::leaf(region, index);
    let leaf = word(host, node);
    let mut seen = leaf.load();
    loop {
        if seen & bits != bits {
            return false;
        }
        match leaf.compare_exchange(seen, seen & !bits) {
            Ok(_) => {
                if seen == bits {
                    clear(host, node);
                }
                return true;
            }
            Err(now) => seen = now,
        }
    }
}

/// Frees `bits` of a leaf word. Aborts if any of them is already free: that is a double free.
pub(crate) fn release<H: Host>(host: &H, region: RegionId, index: u32, bits: u64) {
    let node = Node::leaf(region, index);
    let old = word(host, node).fetch_or(bits);
    if old & bits != 0 {
        host.abort("double free: an object was freed while already free");
    }
    up(host, node, old == 0);
}

/// Walks up from `node`, whose word is not empty, making sure every summary bit on the way to
/// the top is set. `must` means the bit above `node` is to be set without reading it first,
/// because the word was empty before this thread's write.
fn up<H: Host>(host: &H, mut node: Node, mut must: bool) {
    let layout = host.layout();
    while let Some((above, bit)) = parent(layout, node) {
        let summary = word(host, above);
        if must {
            must = summary.fetch_or(bit) == 0;
        } else if summary.load() & bit == 0 {
            // Set it on the next turn, as an OR.
            must = true;
            continue;
        }
        node = above;
    }
}

/// `node`'s word was seen empty by this thread, which emptied it or found it empty under a set
/// bit: clear the bit above it and read the word again.
fn clear<H: Host>(host: &H, mut node: Node) {
    let layout = host.layout();
    while let Some((above, bit)) = parent(layout, node) {
        let left = word(host, above).fetch_and(!bit) & !bit;
        if word(host, node).load() != 0 {
            up(host, node, true);
            return;
        }
        if left != 0 {
            return;
        }
        node = above;
    }
}
