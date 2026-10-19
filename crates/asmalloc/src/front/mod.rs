//! The front end: per-CPU slabs of magazines, and the routines that work on them.
//!
//! Each CPU number has a `Slab` in the static `SLABS`. A slab's hot word for a small class
//! is the address of the next free slot of the magazine loaded for that class, or zero, or a
//! marker; its cold part holds the CPU's spare magazines and the counters the idle check and
//! the drain use. The array is zero-initialized, and zero is a valid slab with no magazines,
//! so nothing needs initializing before the first call.
//!
//! Only the `Routines` write a slab, each with one restartable sequence that commits with a
//! single store, and the drain (`depot::drain`). The routines are assembly generated from Lean
//! into `generated/` and wrapped as `Real`; `Twin` implements the same specifications in
//! Rust for every environment where assembly cannot run (Miri, Kani and the simulated-OS
//! tests), over a simulated processor (`os::Machine`) that tests control. Which one a heap uses
//! is chosen where the heap is made, by build configuration.
//!
//! The layout constants here must equal those the Lean specification gives the generated code
//! (`generated/*/slab.rs`); a test compares them.

#[cfg(all(target_env = "musl", target_arch = "x86_64", not(miri), not(kani)))]
mod area;
#[cfg(test)]
pub(crate) mod check;
#[cfg(all(test, target_os = "linux", target_arch = "x86_64", not(miri)))]
mod differential;
// The generated routines and their wrapper, wherever assembly can run. The generated code names
// its surroundings by path, so they are provided here: `SLABS` and `__rseq_offset`.
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
#[allow(clippy::undocumented_unsafe_blocks)]
mod generated;
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
mod real;
#[cfg(all(test, target_os = "linux", target_arch = "x86_64", not(miri)))]
mod rseq_offset_check;
/// The generated C entry points' module, for the C layer.
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub(crate) use generated::x86_64::centry as generated_centry;
mod routines;
mod slab;
pub(crate) mod sync;
mod twin;
#[cfg(kani)]
mod verify;
#[cfg(kani)]
mod verify_slow;

// The module is public only for `SLABS` and `__rseq_offset`, which the C routines that
// `export_c_abi!` defines in another crate name; the rest is the crate's own unless the test
// kit and the benchmarks ask for it (feature `internals`).
#[cfg(not(feature = "internals"))]
pub(crate) use routines::{NO_CPU, Routines, StackPop, StackPush, StoreIfEqual};
#[cfg(feature = "internals")]
pub use routines::{NO_CPU, Routines, StackPop, StackPush, StoreIfEqual};
#[cfg(not(feature = "internals"))]
pub(crate) use slab::{Cpu, Field, SLAB_BYTES, SLAB_COUNT, SLAB_SHIFT, Slab, Slabs, Spare};
#[cfg(feature = "internals")]
pub use slab::{Cpu, Field, SLAB_BYTES, SLAB_COUNT, SLAB_SHIFT, Slab, Slabs, Spare};
#[cfg(not(feature = "internals"))]
pub(crate) use twin::Twin;
#[cfg(feature = "internals")]
pub use twin::Twin;

// SAFETY: FRONT-01. glibc declares `extern const ptrdiff_t __rseq_offset;` (`sys/rseq.h`); on
// musl the word is the crate's own, generated with its area (`area`). The exported C routines
// name it through this public path.
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
unsafe extern "C" {
    /// The offset of the thread's rseq area from the thread pointer, which the generated
    /// routines read: glibc's, or on musl, which has none, the crate's own word, under a name of
    /// its own so that it can never be taken for a C library's. Public only so that the C
    /// routines the export macros define in another crate can name it; reading it is all
    /// anyone can do.
    #[doc(hidden)]
    #[cfg_attr(target_env = "musl", link_name = "asmalloc_rseq_offset")]
    pub safe static __rseq_offset: isize;
}
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub(crate) use real::Real;

use crate::classes::{SMALL_COUNT, SmallClass};
use core::mem::offset_of;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// The number of entries in the masks' table: one per value of a class byte, so that a class
/// read from the region table indexes it whatever it is, and finds zero, which sends the
/// generated C routines to their slow paths, for every byte that is not a small class.
pub(crate) const MASK_COUNT: usize = crate::spec::MASK_COUNT;

/// Where the masks' table starts within its page: past every field of a slab, so that no entry
/// shares a page offset with a slab word (a load at the page offset of a store in flight waits
/// for it: `docs/research/zen5-roofline.md`, section 14).
const MASK_PAGE_OFFSET: usize = 2048;

/// Where the mirrors of the heap words start within the masks' page, and where the region table
/// starts within the page after it: off the page offsets of the slab words the fast paths write,
/// since the C frees read them on every call.
const PUBLISHED_PAGE_OFFSET: usize = 1024;

const REGION_PAGE_OFFSET: usize = 2048;

/// The number of entries of the region table the C frees read: one per region.
pub(crate) const REGION_COUNT: usize = crate::space::MAX_REGIONS;

/// The slabs, one per CPU number, and after them what the C entry points read at fixed distances
/// from a class's word in slab 0: the masks, the published heap words and the region table.
///
/// A C routine forms the address of its class's word in slab 0 and reads the class's mask at a
/// fixed distance from it, `MASK_DISP`, which the routines were proved for, and the C frees read the published words and the region table at `SHIFT_DISP`,
/// `BASE_DISP` and `REGION_DISP` from `SLABS`. One static keeps those distances fixed by
/// construction. Everything is zero at load: an untouched slab costs no memory, a zero mask sends
/// every generated C routine to its slow path, which writes the masks ([`publish_masks`]),
/// and zero published words and region bytes send every C free to its slow path, which writes
/// them (`c::mirror`).
///
/// The type is public, with no public field or method, only so that the C routines the export
/// macros define in another crate can name [`SLABS`]: code outside the crate can take its address
/// and nothing else.
#[doc(hidden)]
#[repr(C, align(4096))]
pub struct Front {
    slabs: Slabs<[Slab; SLAB_COUNT]>,
    _gap0: [u8; PUBLISHED_PAGE_OFFSET],
    /// The C frees' mirror of the heap's region shift (in the low 32 bits) and its base address
    /// shifted right by it.
    published: [AtomicU64; 2],
    _gap1: [u8; MASK_PAGE_OFFSET - PUBLISHED_PAGE_OFFSET - 16],
    masks: [AtomicU64; MASK_COUNT],
    _gap2: [u8; REGION_PAGE_OFFSET],
    /// A copy of the region table's class bytes for small classes, kept in step by the C slow
    /// paths (`c::mirror`).
    regions: [AtomicU8; REGION_COUNT],
    _gap3: [u8; SLAB_BYTES - REGION_PAGE_OFFSET],
}

impl Front {
    const fn new() -> Front {
        Front {
            slabs: Slabs::new(),
            _gap0: [0; PUBLISHED_PAGE_OFFSET],
            published: [const { AtomicU64::new(0) }; 2],
            _gap1: [0; MASK_PAGE_OFFSET - PUBLISHED_PAGE_OFFSET - 16],
            masks: [const { AtomicU64::new(0) }; MASK_COUNT],
            _gap2: [0; REGION_PAGE_OFFSET],
            regions: [const { AtomicU8::new(0) }; REGION_COUNT],
            _gap3: [0; SLAB_BYTES - REGION_PAGE_OFFSET],
        }
    }

    /// The slabs.
    #[inline(always)]
    pub(crate) fn slabs(&self) -> &Slabs {
        &self.slabs
    }

    /// The address of a class's hot word in slab 0, which the routines take.
    #[inline(always)]
    pub(crate) fn word(&self, class: SmallClass) -> usize {
        core::ptr::from_ref(&self.slabs).addr() + Field::hot(class).offset()
    }

    /// The heap words the C frees read: the region shift and the base's region index,
    /// `(shift, base >> shift)`, both zero until published.
    #[cfg_attr(any(miri, kani), allow(dead_code))]
    pub(crate) fn published(&self) -> (&AtomicU64, &AtomicU64) {
        (&self.published[0], &self.published[1])
    }

    /// The copy of the region table's class bytes the C frees read.
    #[cfg_attr(any(miri, kani), allow(dead_code))]
    pub(crate) fn regions(&self) -> &[AtomicU8; REGION_COUNT] {
        &self.regions
    }
}

/// The distance from a class's word in slab 0 to its mask, by the layout of [`Front`].
#[cfg_attr(any(miri, kani), allow(dead_code))]
pub(crate) const MASK_DISP: usize = offset_of!(Front, masks) - offset_of!(Front, slabs);

/// The distances from `SLABS` to the first mirror's words and the region table, by the layout of
/// [`Front`]; mirror `i`'s words are `16 i` bytes past the first's.
#[cfg_attr(any(miri, kani), allow(dead_code))]
pub(crate) const SHIFT_DISP: usize = offset_of!(Front, published) - offset_of!(Front, slabs);
#[cfg_attr(any(miri, kani), allow(dead_code))]
pub(crate) const BASE_DISP: usize = SHIFT_DISP + 8;
#[cfg_attr(any(miri, kani), allow(dead_code))]
pub(crate) const REGION_DISP: usize = offset_of!(Front, regions) - offset_of!(Front, slabs);

// The layout is the one the routines were proved for.
const _: () = {
    use crate::spec;
    assert!(MASK_DISP == spec::MASK_DISP);
    assert!(SHIFT_DISP == spec::SHIFT_DISP && BASE_DISP == spec::BASE_DISP);
    // The routines were proved for one mirror, C's, at `SHIFT_DISP`.
    assert!(spec::MIRROR_COUNT == 1 && size_of::<[AtomicU64; 2]>() == spec::MIRROR_BYTES);
    assert!(REGION_DISP == spec::REGION_DISP && REGION_COUNT == spec::REGION_COUNT);
    // Every small class's hot word is one the routines may address.
    assert!(Slab::HOT_BYTES / 8 <= spec::CLASS_LIMIT);
};

const _: () = {
    assert!(offset_of!(Front, slabs) == 0);
    // What the C frees read on every call is off the page offsets of the slab words the fast
    // paths write: at bytes 0 to 15 of a 32-byte chunk, where no hot word is (`slab`; the
    // spare heads, which only slow paths write, may share them).
    assert!(SHIFT_DISP.is_multiple_of(32) && BASE_DISP + 8 <= SHIFT_DISP + 16);
    assert!((BASE_DISP + 8) % SLAB_BYTES <= MASK_PAGE_OFFSET);
    assert!(
        REGION_DISP % SLAB_BYTES == REGION_PAGE_OFFSET && REGION_PAGE_OFFSET >= Slab::USED_BYTES
    );
    assert!(size_of::<Front>() == SLAB_COUNT * SLAB_BYTES + 3 * SLAB_BYTES);
    assert!(offset_of!(Front, masks) % SLAB_BYTES == MASK_PAGE_OFFSET);
    assert!(MASK_PAGE_OFFSET + MASK_COUNT * 8 <= SLAB_BYTES);
    // No mask shares a page offset with any field of a slab.
    assert!(Slab::USED_BYTES <= MASK_PAGE_OFFSET);
    assert!(Slab::HOT_BYTES / 8 <= MASK_COUNT);
};

/// The slabs, and after them what the routines read at fixed distances from them. The generated
/// routines name it as `front::SLABS`, whose address is the first slab's.
///
/// Public, hidden, only so that the C routines the export macros define in another crate can
/// name it: [`Front`] has no public field or method, so code outside the crate can take its
/// address and nothing else. A twin or routine wrapper made outside the crate could otherwise
/// write the slabs without restarting, behind the heap's back.
// Only the generated routines use it, which Miri and Kani do not build.
#[doc(hidden)]
#[cfg_attr(any(miri, kani), allow(dead_code))]
pub static SLABS: Front = Front::new();

/// The slot mask of each small class's magazines, indexed by the position of the class's hot word
/// (`SmallClass::word`); zero for every other byte. The inlined routines are given their class's
/// mask from here, so a constant class folds it to a constant; the generated C routines read the
/// copy in [`SLABS`], at the same index, `MASK_DISP` bytes above the word.
///
/// An inlined routine with a class known only at run time loads its mask on every call, so the
/// table sits at page offsets no slab field has, as the class lookup does
/// (`classes::BY_SIZE_PAGE_OFFSET`).
static MASKS: PlacedMasks = PlacedMasks {
    _pad: [0; MASKS_PAGE_OFFSET],
    masks: MASK_TABLE,
};

/// Where [`MASKS`] starts within its page.
const MASKS_PAGE_OFFSET: usize = 2048;

/// [`MASKS`]'s table, [`MASKS_PAGE_OFFSET`] bytes into a page-aligned static.
#[repr(C, align(4096))]
struct PlacedMasks {
    _pad: [u8; MASKS_PAGE_OFFSET],
    masks: [u32; MASK_COUNT],
}

// The page-offset rule (`docs/research/zen5-roofline.md`, section 16): a load whose bytes overlap,
// in the low 12 bits of the address, a hot word that the previous call stored and that is still
// in flight can wait for that store, and since the hot word's address comes from what the call
// loaded, a loop of calls then runs one at a time. So nothing a fast path reads or writes on
// every call shares a page offset with a hot word. For what the crate places, that is checked
// here. For what others place it rests on alignment: the kernel requires a thread's
// `struct rseq` to be 32-byte aligned (`include/uapi/linux/rseq.h`), and the routines touch
// only its `cpu_id` (read) and `rseq_cs` (written), at bytes 4 to 15 of their 32-byte chunk,
// never `node_id` or `mm_cid`; and the executable's copy of glibc's `__rseq_offset` is an
// 8-byte word as aligned as glibc's own, 16 bytes on the glibc measured. What is not covered is
// listed in that section: a GOT slot, an only 8-aligned `__rseq_offset`, and the tables' wrapped
// entries (sizes above 16 KiB, sized frees of 2 KiB and up, the region table's top half).
// `rseq_offset_check` (tests) and `scripts/rseq-offset-check` look at the linked binaries.

/// Whether page offset `b` is a byte of some small class's hot word.
const HOT_PAGE_BYTES: [bool; SLAB_BYTES] = {
    let mut hot = [false; SLAB_BYTES];
    let mut ordinal = 0;
    while ordinal < SMALL_COUNT {
        if let Some(class) = SmallClass::from_ordinal(ordinal) {
            let at = Field::hot(class).offset() % SLAB_BYTES;
            let mut b = 0;
            while b < 8 {
                hot[at + b] = true;
                b += 1;
            }
        }
        ordinal += 1;
    }
    hot
};

/// Whether no byte of `len` bytes at page offset `at` (wrapping round the page) is a hot word's.
const fn clear_of_hot(at: usize, len: usize) -> bool {
    let mut b = 0;
    while b < len {
        if HOT_PAGE_BYTES[(at + b) % SLAB_BYTES] {
            return false;
        }
        b += 1;
    }
    true
}

const _: () = {
    use crate::classes::{BY_SIZE_CLEAR, BY_SIZE_PAGE_OFFSET, QUANTUM};
    use crate::spec;
    assert!(SLAB_BYTES == 4096 && align_of::<Front>() == SLAB_BYTES);
    assert!(align_of::<PlacedMasks>() == SLAB_BYTES);
    assert!(offset_of!(PlacedMasks, masks) == MASKS_PAGE_OFFSET);
    // Others' placements: every `cpu_id` and `rseq_cs` of a 32-byte-aligned area, and every
    // 16-aligned 8-byte word.
    let mut at = 0;
    while at < SLAB_BYTES {
        assert!(clear_of_hot(at + spec::RSEQ_CPU_ID, 4) && clear_of_hot(at + spec::RSEQ_CS, 8));
        assert!(clear_of_hot(at, 8) && clear_of_hot(at + 16, 8));
        at += 32;
    }
    // The published words the C frees read.
    assert!(clear_of_hot(SHIFT_DISP % SLAB_BYTES, spec::MIRROR_BYTES));
    // The masks, the C routines' copy and the inlined routines' table, at each class's position.
    let mut ordinal = 0;
    while ordinal < SMALL_COUNT {
        if let Some(class) = SmallClass::from_ordinal(ordinal) {
            assert!(clear_of_hot(
                (MASK_DISP + Field::hot(class).offset()) % SLAB_BYTES,
                8
            ));
            assert!(clear_of_hot(MASKS_PAGE_OFFSET + 4 * class.word(), 4));
        }
        ordinal += 1;
    }
    // The size tables, for every size up to 16 KiB (`BY_SIZE_CLEAR`); the C tables are
    // `spec::TABLE_OFF` into page-aligned statics (`c::table`). And the region table's lower
    // half.
    assert!(BY_SIZE_PAGE_OFFSET + BY_SIZE_CLEAR / QUANTUM == SLAB_BYTES);
    assert!(clear_of_hot(BY_SIZE_PAGE_OFFSET, BY_SIZE_CLEAR / QUANTUM));
    assert!(clear_of_hot(spec::TABLE_OFF, BY_SIZE_CLEAR / 8 + 1));
    assert!(clear_of_hot(spec::TABLE_OFF, spec::CALLOC_LEN));
    assert!(clear_of_hot(spec::TABLE_OFF, SLAB_BYTES - spec::TABLE_OFF));
    assert!(clear_of_hot(
        REGION_DISP % SLAB_BYTES,
        SLAB_BYTES - REGION_PAGE_OFFSET
    ));
};

const MASK_TABLE: [u32; MASK_COUNT] = {
    let mut masks = [0; MASK_COUNT];
    let mut ordinal = 0;
    while ordinal < SMALL_COUNT {
        if let Some(class) = SmallClass::from_ordinal(ordinal) {
            masks[class.word()] = class.magazine().mask() as u32;
        }
        ordinal += 1;
    }
    masks
};

/// The mask of a class's magazines.
#[inline(always)]
pub(crate) fn mask(class: SmallClass) -> u32 {
    MASKS.masks[class.word()]
}

/// The last position with a mask, which [`publish_masks`] writes last and so checks first.
const LAST_MASKED: usize = {
    let mut last = MASK_COUNT - 1;
    while MASK_TABLE[last] == 0 {
        last -= 1;
    }
    last
};

/// Writes the masks the generated C routines read, if they have not been. Every thread writes
/// the same values, so a race is harmless; until a class's entry is written its routines miss
/// and land in a slow path, which calls this.
#[cfg_attr(any(miri, kani), allow(dead_code))]
pub(crate) fn publish_masks() {
    if SLABS.masks[LAST_MASKED].load(Ordering::Relaxed) != 0 {
        return;
    }
    for (entry, &mask) in SLABS.masks.iter().zip(MASK_TABLE.iter()) {
        entry.store(mask as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_table_holds_each_class_mask_at_its_words_position_and_zero_elsewhere() {
        for class in SmallClass::all() {
            assert_eq!(mask(class) as usize, class.magazine().mask());
        }
        let small = SmallClass::all().count();
        assert_eq!(MASKS.masks.iter().filter(|&&m| m != 0).count(), small);
        assert_eq!(MASKS.masks[0], 0);
    }

    /// The placement the constant assertions describe, on the addresses the build gave: no
    /// mask, and no class lookup entry up to `BY_SIZE_CLEAR` bytes, overlaps a hot word's page
    /// offset.
    #[test]
    fn the_inlined_lookups_are_off_every_hot_words_page_offset() {
        use crate::classes::{BY_SIZE_CLEAR, by_size_entry};
        let hot: std::vec::Vec<usize> = SmallClass::all()
            .map(|class| Field::hot(class).offset() % SLAB_BYTES)
            .collect();
        let clear = |addr: usize, len: usize| {
            let at = addr % SLAB_BYTES;
            hot.iter().all(|&word| at + len <= word || word + 8 <= at)
        };
        for class in SmallClass::all() {
            let entry = core::ptr::from_ref(&MASKS.masks[class.word()]).addr();
            assert!(clear(entry, 4), "{class:?}");
        }
        for size in 1..=BY_SIZE_CLEAR {
            assert!(clear(by_size_entry(size), 1), "{size}");
        }
    }

    // The static's 32 MiB of atomics exceed what Miri can track; under Miri the twins run on
    // small slab arrays instead.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn the_static_slabs_are_one_per_cpu_number_and_the_masks_follow() {
        let slabs = SLABS.slabs();
        assert_eq!(slabs.len(), SLAB_COUNT);
        assert_eq!(size_of_val(&SLABS.slabs), SLAB_COUNT << SLAB_SHIFT);
        let base = core::ptr::from_ref(&SLABS).addr();
        assert_eq!(base % SLAB_BYTES, 0);
        assert_eq!(core::ptr::from_ref(slabs).addr(), base);
        for class in SmallClass::all() {
            let word = SLABS.word(class);
            let entry = core::ptr::from_ref(&SLABS.masks[Field::hot(class).index()]).addr();
            assert_eq!(entry, word + MASK_DISP);
            // Not at a slab word's page offset.
            assert!(entry % SLAB_BYTES >= Slab::USED_BYTES);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn publishing_writes_every_small_mask_once_and_nothing_else() {
        publish_masks();
        publish_masks();
        for (i, entry) in SLABS.masks.iter().enumerate() {
            assert_eq!(entry.load(Ordering::Relaxed), MASK_TABLE[i] as u64);
        }
    }
}
