//! Slabs: one block of state per CPU number.
//!
//! A slab is [`SLAB_BYTES`] bytes. Its first words are the hot words, one per class, the word of
//! class `c` (its [`Class::index`](crate::classes::Class::index)) at byte `8 c`; the routines
//! address them as `SLABS + (cpu << SLAB_SHIFT) + 8 c`. Word 0 is never used, so that the class
//! index needs no adjustment. The cold part follows on its own cache lines: the heads of the
//! spare stacks (one stack of non-empty and one of empty magazines per class), the activity
//! counter and the drain claim, and on a line of its own the idle check's last sighting.
//!
//! Every field is an atomic word and the all-zero slab is valid: no magazines, empty spare
//! stacks, no activity and no claim.

use super::sync::{AtomicPtr, AtomicU64};
use crate::classes::{SMALL_COUNT, SmallClass};
use core::ptr;
use core::sync::atomic::Ordering;

/// The number of slabs: one for every CPU number the kernel can produce on x86-64.
pub const SLAB_COUNT: usize = 8192;
/// `log2` of [`SLAB_BYTES`].
pub const SLAB_SHIFT: u32 = 12;
/// The bytes of a slab, and its alignment.
pub const SLAB_BYTES: usize = 1 << SLAB_SHIFT;

/// Hot words per slab: room for the class indexes `1..WORDS`.
const WORDS: usize = 64;
/// Spare-stack heads per kind, indexed like the hot words.
const HEADS: usize = 64;

const _: () = assert!(SMALL_COUNT < WORDS && SMALL_COUNT < HEADS);
const _: () = assert!(size_of::<Slab>() == SLAB_BYTES && align_of::<Slab>() == SLAB_BYTES);
// The fast path loads only the hot words; the cold words are on lines of their own.
const _: () = assert!(core::mem::offset_of!(Slab, activity) % 64 == 0);
const _: () = assert!(core::mem::offset_of!(Slab, seen) % 64 == 0);
const _: () = assert!(core::mem::offset_of!(Slab, seen) >= core::mem::offset_of!(Slab, claim) + 8);

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
        Field(8 * class.class().index() as u16)
    }

    /// The head of one of a class's spare stacks.
    #[inline(always)]
    pub const fn spare(class: SmallClass, kind: Spare) -> Field {
        let base = match kind {
            Spare::Full => WORDS,
            Spare::Empty => WORDS + HEADS,
        };
        Field(8 * (base + class.class().index() as usize) as u16)
    }

    /// The byte offset in the slab.
    #[inline(always)]
    pub const fn offset(self) -> usize {
        self.0 as usize
    }

    /// The index in a slab's array of pointer fields.
    #[inline(always)]
    const fn index(self) -> usize {
        self.0 as usize / 8
    }
}

/// One CPU's slab. See the module documentation.
#[repr(C, align(4096))]
pub struct Slab {
    /// The hot words, then the full heads, then the empty heads.
    fields: [AtomicPtr<u8>; WORDS + 2 * HEADS],
    activity: AtomicU64,
    claim: AtomicU64,
    /// On a line of its own: other CPUs' idle checks write it, and should not take the line
    /// that this CPU's slow paths count their activity in.
    seen: OwnLine,
}

/// A word alone on its cache line.
#[repr(C, align(64))]
struct OwnLine(AtomicU64);

impl Slab {
    /// The bytes at the start of a slab that hold its hot words, which the fast paths write.
    pub(crate) const HOT_BYTES: usize = 8 * WORDS;

    /// The bytes at the start of a slab that hold its fields; the rest of its page is unused.
    pub(crate) const USED_BYTES: usize = core::mem::offset_of!(Slab, seen) + size_of::<OwnLine>();

    /// A slab with nothing in it.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Slab {
        Slab {
            fields: [const { AtomicPtr::new(ptr::null_mut()) }; WORDS + 2 * HEADS],
            activity: AtomicU64::new(0),
            claim: AtomicU64::new(0),
            seen: OwnLine(AtomicU64::new(0)),
        }
    }

    /// A field. A [`Field`] is always in bounds; the mask lets the compiler drop the check.
    #[inline(always)]
    pub(crate) fn field(&self, field: Field) -> &AtomicPtr<u8> {
        &self.fields[field.index() % (WORDS + 2 * HEADS)]
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

    /// Counts one slow-path entry on this slab's CPU, and returns the new count. A hint for
    /// the idle check, so a lost increment does no harm.
    #[inline]
    pub fn note_activity(&self) -> u64 {
        self.activity
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
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
    fn the_hot_word_is_at_eight_times_the_class_index() {
        let slab = Slab::new();
        for class in SmallClass::all() {
            let field = slab.field(Field::hot(class));
            let offset = ptr::from_ref(field).addr() - ptr::from_ref(&slab).addr();
            assert_eq!(offset, 8 * class.class().index() as usize);
            assert_eq!(offset, Field::hot(class).offset());
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
