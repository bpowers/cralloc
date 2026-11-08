//! Slabs: one block of state per CPU number.
//!
//! A slab is [`SLAB_BYTES`] bytes. It begins with a 32-byte chunk per class, chunk `c` for class
//! index `c` (chunk 0 unused): the heads of the class's two spare stacks (one of non-empty and
//! one of empty magazines) at bytes 0 to 15, at bytes 16 to 23 the class's stock word, which
//! only this CPU's slow paths read and write with plain loads and stores (`depot::stock`; no
//! routine is ever aimed at it), and its hot word at bytes 24 to 31 ([`Field::hot`]). The
//! routines address the word at position `p` as `SLABS + (cpu << SLAB_SHIFT) + 8 p`, and the C
//! size tables and the region table give that position, `4 c + 3`. A load at the page offset of
//! a hot word stored just before can wait for that store as if it were the same location
//! (`docs/research/roofline.md`, "A load at the page offset of a store in flight can stall the
//! whole loop"), so the hot words keep off the page offsets of what every call reads or writes
//! and others place: a thread's rseq area, which the kernel requires to be 32-byte aligned, has
//! `cpu_id` and `rseq_cs` at bytes 4 to 15 of a 32-byte chunk wherever the C library puts it;
//! and an 8-byte variable aligned to 16, as the executable's copy of glibc's `__rseq_offset` is
//! when glibc's own is, lies at bytes 0 to 7 or 16 to 23. `front` asserts the rule for
//! everything the crate places itself. One class to a chunk keeps the word's address one shift
//! from the class index, where two to a chunk cost a run-time-size pair two cycles of
//! arithmetic. The cold part follows on its own cache lines: the activity counter (bytes its
//! slow paths moved), the count of its heartbeats, the drain claim, the count of full magazines
//! the CPU's frees have retired, where the age limit's sweep of the spare stacks is, and the
//! classes that may keep stock; then, on lines of their own that only this CPU's slow paths
//! touch, per class a stamp of when the loaded magazine was loaded; and on a line of its own
//! the idle check's last sighting.
//!
//! Every field is an atomic word and the all-zero slab is valid: no magazines, empty spare
//! stacks, no activity, no claim, nothing retired, no sweep and no stamps.

use super::sync::{AtomicPtr, AtomicU64};
use crate::classes::{SMALL_COUNT, SmallClass, WORD_START, WORD_STRIDE};
use crate::lined::LINE;
use core::ptr;
use core::sync::atomic::Ordering;

/// The number of slabs: one for every CPU number a kernel can produce, 8192 on x86-64 and 4096 on
/// aarch64 (`docs/research/linux-rseq-contract.md`, "aarch64").
pub const SLAB_COUNT: usize = crate::spec::SLAB_COUNT;
/// `log2` of [`SLAB_BYTES`].
pub const SLAB_SHIFT: u32 = crate::spec::SLAB_SHIFT;
/// The bytes of a slab, and its alignment.
pub const SLAB_BYTES: usize = crate::spec::SLAB_BYTES;
const _: () = assert!(SLAB_BYTES == 1 << SLAB_SHIFT);

/// One 32-byte chunk per class index `0..=SMALL_COUNT` (chunk 0 unused, and one more if needed
/// for the cold fields to start on a cache line): its full head, its empty head, its stock word,
/// and its hot word at bytes 24 to 31.
const CHUNKS: usize = (SMALL_COUNT + 1).next_multiple_of(LINE / 32);
/// All the pointer fields: four to a chunk.
const FIELDS: usize = 4 * CHUNKS;

/// The position of class index `index`'s hot word among the slab's words: bytes 24 to 31 of
/// chunk `index`, as a [`SmallClass`] holds it.
const fn hot_position(index: usize) -> usize {
    WORD_STRIDE * index + WORD_START
}
const _: () = assert!(WORD_STRIDE == 4 && WORD_START == 3);

const _: () = assert!(hot_position(SMALL_COUNT) < FIELDS && hot_position(1) == 7);
/// The bytes of its 32-byte chunk each hot word fills (see the module documentation;
/// `front` checks what the crate places against them).
pub(crate) const HOT_CHUNK_BYTES: core::ops::Range<usize> = 24..32;
const _: () = {
    let mut index = 1;
    while index <= SMALL_COUNT {
        assert!((8 * hot_position(index)) % 32 == HOT_CHUNK_BYTES.start);
        index += 1;
    }
};
const _: () = assert!(size_of::<Slab>() == SLAB_BYTES && align_of::<Slab>() == SLAB_BYTES);
// The fast path loads only the hot words; the cold words are on lines of their own.
const _: () = assert!(core::mem::offset_of!(Slab, activity) % LINE == 0);
const _: () = assert!(core::mem::offset_of!(Slab, seen) % LINE == 0);
const _: () = assert!(core::mem::offset_of!(Slab, seen) >= core::mem::offset_of!(Slab, claim) + 8);
const _: () = assert!(core::mem::offset_of!(Slab, stamps) % LINE == 0);
const _: () = assert!(core::mem::offset_of!(Slab, stamps) >= FIELDS * 8);

/// Which spare stack of a class.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Spare {
    /// Magazines with at least one object: what allocation wants.
    Full,
    /// Magazines with no objects: what free wants.
    Empty,
}

/// A pointer-sized field of a slab that the routines can be aimed at: a hot word or a spare
/// head. The value is its byte offset in the slab, which is what the routines take.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Field(u16);

impl Field {
    /// The hot word of a class: the address of the next free slot of its loaded magazine.
    #[inline(always)]
    pub const fn hot(class: SmallClass) -> Field {
        Field(8 * class.word() as u16)
    }

    /// The head of one of a class's spare stacks.
    #[inline(always)]
    pub const fn spare(class: SmallClass, kind: Spare) -> Field {
        let within = match kind {
            Spare::Full => 0,
            Spare::Empty => 1,
        };
        Field(8 * (class.word() - WORD_START + within) as u16)
    }

    /// The byte offset in the slab.
    #[inline(always)]
    pub const fn offset(self) -> usize {
        self.0 as usize
    }

    /// The index in a slab's array of pointer fields: for a hot word, its position, which the C
    /// size tables and the region table hold for the class and the C routines turn into the
    /// word's address.
    #[inline(always)]
    pub(crate) const fn index(self) -> usize {
        self.0 as usize / 8
    }
}

/// One CPU's slab. See the module documentation.
#[repr(C, align(4096))]
pub struct Slab {
    /// A chunk per class: its full head, its empty head, its hot word, and its stock word.
    fields: [AtomicPtr<u8>; FIELDS],
    activity: AtomicU64,
    /// The heartbeats this CPU's slow paths have run: the turn of the rotating chores.
    beats: AtomicU64,
    claim: AtomicU64,
    retired: AtomicU64,
    sweep: AtomicU64,
    /// The classes that may keep stock here, one bit per ordinal (`depot::stock`).
    stocked: AtomicU64,
    stamps: Stamps,
    /// On a line of its own: other CPUs' idle checks write it, and should not take the line
    /// that this CPU's slow paths count their activity in.
    seen: OwnLine,
}

/// Per class, indexed by class index like the hot words, when the magazine loaded there was
/// loaded (`depot::age`). On lines of their own, away from the hot words the fast path loads
/// and from the sighting other CPUs write. Only the class indexes in use have one, so that the
/// slab's fields stay below the masks' page offset.
#[cfg_attr(target_arch = "aarch64", repr(C, align(128)))]
#[cfg_attr(not(target_arch = "aarch64"), repr(C, align(64)))]
struct Stamps([AtomicU64; STAMPS]);

/// Stamps per slab: one for each class index up to the largest small class's.
const STAMPS: usize = SMALL_COUNT + 1;

/// A word alone on its cache line.
#[cfg_attr(target_arch = "aarch64", repr(C, align(128)))]
#[cfg_attr(not(target_arch = "aarch64"), repr(C, align(64)))]
struct OwnLine(AtomicU64);

const _: () = assert!(align_of::<Stamps>() == LINE && align_of::<OwnLine>() == LINE);

impl Slab {
    /// The bytes at the start of a slab that hold its hot words, which the fast paths write,
    /// and the spare heads beside them.
    pub(crate) const HOT_BYTES: usize = 8 * FIELDS;

    /// The bytes at the start of a slab that hold every field but the idle check's sighting,
    /// which other CPUs store to once an interval, far too rarely for a load at its page offset
    /// to meet the store. On lines of 128 bytes the sighting's line lies past what the C routines'
    /// tables keep clear; the rest of the slab is unused.
    pub(crate) const STORED_BYTES: usize = core::mem::offset_of!(Slab, seen);

    /// A slab with nothing in it.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Slab {
        Slab {
            fields: [const { AtomicPtr::new(ptr::null_mut()) }; FIELDS],
            activity: AtomicU64::new(0),
            beats: AtomicU64::new(0),
            claim: AtomicU64::new(0),
            retired: AtomicU64::new(0),
            sweep: AtomicU64::new(0),
            stocked: AtomicU64::new(0),
            stamps: Stamps([const { AtomicU64::new(0) }; STAMPS]),
            seen: OwnLine(AtomicU64::new(0)),
        }
    }

    /// A field. A [`Field`] is always in bounds; the mask lets the compiler drop the check.
    #[inline(always)]
    pub(crate) fn field(&self, field: Field) -> &AtomicPtr<u8> {
        &self.fields[field.index() % FIELDS]
    }

    /// The stock word of `class` (`depot::stock`): bytes 16 to 23 of its chunk, beside the
    /// spare heads that every use of it reads anyway, so it costs no line of its own. A hint,
    /// not a field the routines can be aimed at, and not one the drain exchanges; zero is no
    /// stock.
    #[inline]
    pub(crate) fn stock(&self, class: SmallClass) -> &AtomicPtr<u8> {
        &self.fields[(class.word() - 1) % FIELDS]
    }

    /// The fields the drain exchanges: every small class's hot word and spare heads.
    pub fn drained_fields() -> impl Iterator<Item = Field> {
        SmallClass::all().flat_map(|class| {
            [
                Field::hot(class),
                Field::spare(class, Spare::Full),
                Field::spare(class, Spare::Empty),
            ]
        })
    }

    /// Adds `bytes` moved by a slow path of this slab's CPU to its activity counter, and
    /// returns the counter before and after. A hint for the idle check and the heartbeat's pace
    /// (`depot::heartbeat`), kept with a load and a store: a lost update moves a beat or a drain
    /// a little, and does no harm.
    #[inline]
    pub fn note_activity(&self, bytes: u64) -> (u64, u64) {
        let before = self.activity.load(Ordering::Relaxed);
        let after = before.wrapping_add(bytes);
        self.activity.store(after, Ordering::Relaxed);
        (before, after)
    }

    /// Counts one heartbeat of this slab's CPU, and returns the new count: the heartbeat's turn.
    /// A hint, kept with a load and a store, as the activity counter is.
    #[inline]
    pub fn note_beat(&self) -> u64 {
        let next = self.beats.load(Ordering::Relaxed).wrapping_add(1);
        self.beats.store(next, Ordering::Relaxed);
        next
    }

    /// The count of heartbeats.
    #[cfg(test)]
    pub(crate) fn beats(&self) -> u64 {
        self.beats.load(Ordering::Relaxed)
    }

    /// The activity counter.
    #[inline]
    pub fn activity(&self) -> u64 {
        self.activity.load(Ordering::Relaxed)
    }

    /// The idle check's sighting of the activity counter: the value it last saw and when
    /// (`depot::heartbeat`).
    #[inline]
    pub(crate) fn seen(&self) -> &AtomicU64 {
        &self.seen.0
    }

    /// The drain claim: zero, or the generation of the drainer that holds it.
    #[inline]
    pub(crate) fn claim(&self) -> &AtomicU64 {
        &self.claim
    }

    /// Counts one full magazine retired by a free on this slab's CPU, and returns the new
    /// count (`depot::slow`'s refresh). A hint, kept with a load and a store rather than a
    /// read-modify-write: only this CPU's slow paths write it, and a count lost to a race (a
    /// thread that migrated, a signal handler) only moves which magazine is refreshed.
    #[inline]
    pub(crate) fn count_retired(&self) -> u64 {
        let count = self.retired.load(Ordering::Relaxed).wrapping_add(1);
        self.retired.store(count, Ordering::Relaxed);
        count
    }

    /// Takes back the last count of [`Slab::count_retired`], so that the refresh it made due
    /// falls on the next retirement instead. A hint, as the count is.
    #[inline]
    pub(crate) fn uncount_retired(&self) {
        let count = self.retired.load(Ordering::Relaxed).wrapping_sub(1);
        self.retired.store(count, Ordering::Relaxed);
    }

    /// Where the age limit's sweep of this slab's spare stacks is, and when it began
    /// (`depot::age`). A hint, written with single stores by this CPU's slow paths.
    #[inline]
    pub(crate) fn sweep(&self) -> &AtomicU64 {
        &self.sweep
    }

    /// The classes that may keep stock on this CPU, one bit per ordinal (`depot::stock`): a
    /// hint, written with single stores by this CPU's slow paths, on the line of the activity
    /// counter that every slow path writes.
    #[inline]
    pub(crate) fn stocked(&self) -> &AtomicU64 {
        &self.stocked
    }

    /// The stamp of the magazine loaded for `class` (`depot::age`): zero, or which magazine
    /// was loaded and since when. A hint, written with single stores.
    #[inline]
    pub(crate) fn stamp(&self, class: SmallClass) -> &AtomicU64 {
        &self.stamps.0[class.class().index() as usize % STAMPS]
    }
}

/// A CPU number that has a slab in the array it came from.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Cpu(u32);

impl Cpu {
    /// The number.
    #[inline(always)]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The number as an index.
    #[inline(always)]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// An array of slabs, one per CPU number. The allocator's is `front::SLABS`, with
/// [`SLAB_COUNT`] of them; tests make small ones. `Slabs` (with the default parameter) is the
/// unsized view every user takes, so code is not generic over the count.
#[repr(transparent)]
pub struct Slabs<T: ?Sized = [Slab]> {
    slabs: T,
}

impl<const N: usize> Slabs<[Slab; N]> {
    /// `N` slabs with nothing in them.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Slabs<[Slab; N]> {
        Slabs {
            slabs: [const { Slab::new() }; N],
        }
    }
}

impl Slabs {
    /// The number of slabs.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.slabs.len()
    }

    /// Whether there are none, which leaves every thread without a valid CPU.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.slabs.is_empty()
    }

    /// The CPU a thread's `cpu_id` field names, if it has a slab here. A negative value, which
    /// a thread with no rseq registration reads, is out of range like any other.
    #[inline(always)]
    pub fn cpu(&self, cpu_id: i32) -> Option<Cpu> {
        let raw = cpu_id as u32;
        if (raw as usize) < self.slabs.len() {
            Some(Cpu(raw))
        } else {
            None
        }
    }

    /// A CPU's slab.
    #[inline(always)]
    pub fn slab(&self, cpu: Cpu) -> &Slab {
        // A `Cpu` comes from `cpu`, which checked it against this array's length; slab arrays
        // never shrink. Indexing keeps a `Cpu` from another, longer array from going out of
        // bounds.
        &self.slabs[cpu.index()]
    }

    /// Every CPU with a slab.
    pub fn cpus(&self) -> impl Iterator<Item = Cpu> + '_ {
        (0..self.slabs.len() as u32).map(Cpu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::Class;
    use std::collections::BTreeSet;

    #[test]
    fn fields_are_distinct_and_inside_the_slab() {
        let mut offsets = BTreeSet::new();
        for field in SmallClass::all().flat_map(|class| {
            [
                Field::hot(class),
                Field::spare(class, Spare::Full),
                Field::spare(class, Spare::Empty),
            ]
        }) {
            assert!(field.offset() % 8 == 0 && field.offset() < SLAB_BYTES);
            assert!(offsets.insert(field.offset()), "{field:?}");
        }
        assert_eq!(offsets.len(), 3 * SMALL_COUNT);
        assert_eq!(Slab::drained_fields().count(), 3 * SMALL_COUNT);
        assert!(!offsets.contains(&0), "word 0 is never used");
    }

    #[test]
    fn each_class_has_a_chunk_with_its_hot_word_at_bytes_24_to_31() {
        let slab = Slab::new();
        for class in SmallClass::all() {
            let field = slab.field(Field::hot(class));
            let offset = ptr::from_ref(field).addr() - ptr::from_ref(&slab).addr();
            assert_eq!(offset, Field::hot(class).offset());
            let chunk = 32 * class.class().index() as usize;
            assert_eq!(offset, chunk + 24);
            let stock = ptr::from_ref(slab.stock(class)).addr() - ptr::from_ref(&slab).addr();
            assert_eq!(stock, chunk + 16);
            assert_eq!(Field::spare(class, Spare::Full).offset(), chunk);
            assert_eq!(Field::spare(class, Spare::Empty).offset(), chunk + 8);
            assert!(offset + 8 <= Slab::HOT_BYTES);
        }
        let largest = Class::LARGEST_SMALL.small().unwrap();
        let head = slab.field(Field::spare(largest, Spare::Empty));
        let offset = ptr::from_ref(head).addr() - ptr::from_ref(&slab).addr();
        assert_eq!(offset, Field::spare(largest, Spare::Empty).offset());
    }

    #[test]
    fn cpu_numbers_are_checked_against_the_count() {
        let slabs = Slabs::<[Slab; 3]>::new();
        let slabs: &Slabs = &slabs;
        assert_eq!(slabs.cpu(2).map(Cpu::get), Some(2));
        assert_eq!(slabs.cpu(3), None);
        assert_eq!(slabs.cpu(-1), None);
        assert_eq!(slabs.cpu(-2), None);
        assert_eq!(slabs.cpu(i32::MIN), None);
        assert_eq!(slabs.cpus().count(), 3);
    }
}
