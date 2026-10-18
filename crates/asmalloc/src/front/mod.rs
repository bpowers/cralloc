//! The front end: per-CPU slabs of magazines, and the routines that work on them.
//!
//! Each CPU number has a [`Slab`] in the static `SLABS`. A slab's hot word for a small class
//! is the address of the next free slot of the magazine loaded for that class, or zero, or a
//! marker; its cold part holds the CPU's spare magazines and the counters the idle check and
//! the drain use. The array is zero-initialized, and zero is a valid slab with no magazines,
//! so nothing needs initializing before the first call.
//!
//! Only the [`Routines`] write a slab, each with one restartable sequence that commits with a
//! single store, and the drain (`depot::drain`). The routines are assembly generated from Lean
//! into `generated/` and wrapped as `Real`; [`Twin`] implements the same specifications in
//! Rust for every environment where assembly cannot run (Miri, Kani and the simulated-OS
//! tests), over a simulated processor ([`Machine`](crate::os::Machine)) that tests control. Which one a heap uses
//! is chosen where the heap is made, by build configuration.
//!
//! The layout constants here must equal those the Lean specification gives the generated code
//! (`generated/*/slab.rs`); a test compares them.

#[cfg(test)]
pub(crate) mod check;
// The generated routines and their wrapper, wherever assembly can run. The generated code names
// its surroundings by path, so they are provided here: `SLABS`, `__rseq_offset`,
// `pop_slow` and `push_slow`. The last two are the heap's slow paths, which sit above this
// module; only the generated code's tail jumps reach them through these names.
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
#[allow(clippy::undocumented_unsafe_blocks)]
mod generated;
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
mod real;
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

pub use routines::{NO_CPU, Routines, StackPop, StackPush, StoreIfEqual};
pub use slab::{Cpu, Field, SLAB_BYTES, SLAB_COUNT, SLAB_SHIFT, Slab, Slabs, Spare};
pub use twin::Twin;

#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub(crate) use crate::heap::{pop_slow, push_slow};
// SAFETY: FRONT-01. glibc declares `extern const ptrdiff_t __rseq_offset;` (`sys/rseq.h`); the
// exported C routines name it through this public path.
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
unsafe extern "C" {
    /// glibc's offset of the thread's rseq area from the thread pointer, which the generated
    /// routines read. Public only so that the C routines the export macros define in another
    /// crate can name it; reading it is all anyone can do.
    #[doc(hidden)]
    pub safe static __rseq_offset: isize;
}
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
pub(crate) use real::Real;

use crate::classes::{SMALL_COUNT, SmallClass};
use core::mem::offset_of;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// The number of entries in the masks' table: one per value of a class byte, so that a class
/// read from the region table indexes it whatever it is, and finds zero, which sends the
/// out-of-line routines to their slow paths, for every byte that is not a small class.
pub const MASK_COUNT: usize = 256;

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
pub const REGION_COUNT: usize = crate::space::MAX_REGIONS;

/// The slabs, one per CPU number, and after them what the out-of-line routines and the C entry
/// points read at fixed distances from a class's word in slab 0: the masks, the published heap
/// words and the region table.
///
/// A routine is given (or forms) the address of its class's word in slab 0; the out-of-line ones
/// read the class's mask at a fixed distance from it, `MASK_DISP`, which the routines were proved
/// for, and the C frees read the published words and the region table at `SHIFT_DISP`,
/// `BASE_DISP` and `REGION_DISP` from `SLABS`. One static keeps those distances fixed by
/// construction. Everything is zero at load: an untouched slab costs no memory, a zero mask sends
/// every out-of-line pop and push to its slow path, which writes the masks ([`publish_masks`]),
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

    /// The class whose hot word in slab 0 is at `word`, if it is one: what an out-of-line
    /// routine's slow path is given.
    pub(crate) fn class_of_word(&self, word: usize) -> Option<SmallClass> {
        let offset = word.checked_sub(core::ptr::from_ref(&self.slabs).addr())?;
        if offset % 8 != 0 {
            return None;
        }
        u8::try_from(offset / 8)
            .ok()
            .and_then(crate::classes::Class::from_index)
            .and_then(crate::classes::Class::small)
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
#[cfg(all(target_os = "linux", target_arch = "x86_64", not(miri), not(kani)))]
const _: () = {
    use generated::x86_64::centry as c_proved;
    use generated::x86_64::slab as proved;
    assert!(MASK_DISP == proved::MASK_DISP);
    assert!(MASK_COUNT == proved::MASK_COUNT);
    assert!(SLAB_COUNT == proved::SLAB_COUNT && SLAB_BYTES == proved::SLAB_BYTES);
    assert!(MASK_DISP == c_proved::MASK_DISP && MASK_COUNT == c_proved::MASK_COUNT);
    assert!(SLAB_COUNT == c_proved::SLAB_COUNT && SLAB_BYTES == c_proved::SLAB_BYTES);
    assert!(SHIFT_DISP == c_proved::SHIFT_DISP && BASE_DISP == c_proved::BASE_DISP);
    // The routines were proved for one mirror, C's, at `SHIFT_DISP`.
    assert!(c_proved::MIRROR_COUNT == 1 && size_of::<[AtomicU64; 2]>() == c_proved::MIRROR_BYTES);
    assert!(REGION_DISP == c_proved::REGION_DISP && REGION_COUNT == c_proved::REGION_COUNT);
};

const _: () = {
    assert!(offset_of!(Front, slabs) == 0);
    // What the C frees read on every call is off the page offsets of the slab words the fast
    // paths write (the spare heads, which only slow paths write, may share them).
    assert!(
        SHIFT_DISP % SLAB_BYTES >= Slab::HOT_BYTES
            && (BASE_DISP + 8) % SLAB_BYTES <= MASK_PAGE_OFFSET
    );
    assert!(
        REGION_DISP % SLAB_BYTES == REGION_PAGE_OFFSET && REGION_PAGE_OFFSET >= Slab::USED_BYTES
    );
    assert!(size_of::<Front>() == SLAB_COUNT * SLAB_BYTES + 3 * SLAB_BYTES);
    assert!(offset_of!(Front, masks) % SLAB_BYTES == MASK_PAGE_OFFSET);
    assert!(MASK_PAGE_OFFSET + MASK_COUNT * 8 <= SLAB_BYTES);
    // No mask shares a page offset with any field of a slab.
    assert!(Slab::USED_BYTES <= MASK_PAGE_OFFSET);
    assert!(SMALL_COUNT < MASK_COUNT);
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

/// The slot mask of each small class's magazines, indexed by class index; zero for every other
/// byte. The inlined routines are given their class's mask from here, so a constant class
/// folds it to a constant; the out-of-line routines read the copy in [`SLABS`].
pub static MASKS: [u32; MASK_COUNT] = MASK_TABLE;

const MASK_TABLE: [u32; MASK_COUNT] = {
    let mut masks = [0; MASK_COUNT];
    let mut ordinal = 0;
    while ordinal < SMALL_COUNT {
        if let Some(class) = SmallClass::from_ordinal(ordinal) {
            masks[class.class().index() as usize] = class.magazine().mask() as u32;
        }
        ordinal += 1;
    }
    masks
};

/// The mask of a class's magazines.
#[inline(always)]
pub(crate) fn mask(class: SmallClass) -> u32 {
    MASKS[class.class().index() as usize]
}

/// Writes the masks the out-of-line routines read, if they have not been. Every thread writes
/// the same values, so a race is harmless; until a class's entry is written its routines miss
/// and land in a slow path, which calls this.
#[cfg_attr(any(miri, kani), allow(dead_code))]
pub(crate) fn publish_masks() {
    let last = &SLABS.masks[SMALL_COUNT];
    if last.load(Ordering::Relaxed) != 0 {
        return;
    }
    for (entry, &mask) in SLABS.masks.iter().zip(MASK_TABLE.iter()) {
        entry.store(mask as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::{MARKER_SHIFT, MagazineSize, SLOT_BYTES};
    use crate::os::rseq;
    use std::string::String;
    use std::vec::Vec;

    /// The constants the generated routines were proved against, read from the generated
    /// source: `(name, value)`.
    fn generated_constants(source: &str) -> Vec<(String, u64)> {
        source
            .lines()
            .filter_map(|line| {
                let rest = line.trim().strip_prefix("pub(crate) const ")?;
                let (name, rest) = rest.split_once(':')?;
                let value = rest.split_once('=')?.1.trim().trim_end_matches(';');
                Some((String::from(name.trim()), value.parse().ok()?))
            })
            .collect()
    }

    #[test]
    fn the_layout_matches_what_the_routines_were_proved_against() {
        for source in [
            include_str!("generated/x86_64/slab.rs"),
            include_str!("generated/x86_64_inline/slab.rs"),
        ] {
            let constants = generated_constants(source);
            let get = |name: &str| {
                constants
                    .iter()
                    .find(|(n, _)| n == name)
                    .unwrap_or_else(|| panic!("{name} is not generated"))
                    .1
            };
            assert_eq!(get("SLAB_COUNT"), SLAB_COUNT as u64);
            assert_eq!(get("SLAB_SHIFT"), SLAB_SHIFT as u64);
            assert_eq!(get("SLAB_BYTES"), SLAB_BYTES as u64);
            assert_eq!(get("WORD_BYTES"), SLOT_BYTES as u64);
            assert_eq!(get("MARKER_SHIFT"), MARKER_SHIFT as u64);
            if source.contains("NO_CPU") {
                assert_eq!(get("NO_CPU"), NO_CPU as u64);
            }
            assert_eq!(get("RSEQ_CPU_ID"), rseq::CPU_ID as u64);
            assert_eq!(get("RSEQ_CS"), rseq::RSEQ_CS as u64);
            assert_eq!(get("MASK_DISP"), MASK_DISP as u64);
            assert_eq!(get("MASK_COUNT"), MASK_COUNT as u64);
            // Every small class's hot word is within the words the routines may address, and
            // every magazine size is one they accept, with the mask they expect.
            assert!((SMALL_COUNT as u64) < get("CLASS_LIMIT"));
            for size in MagazineSize::all() {
                let log = size.slots().trailing_zeros() as u64;
                assert!(get("MAG_LOG_MIN") <= log && log <= get("MAG_LOG_MAX"));
                let name = std::format!("MAG_MASK_{}", size.slots());
                assert_eq!(get(&name), size.mask() as u64, "{name}");
            }
        }
    }

    #[test]
    fn the_mask_table_holds_each_class_mask_at_its_index_and_zero_elsewhere() {
        for class in SmallClass::all() {
            assert_eq!(mask(class) as usize, class.magazine().mask());
        }
        let small = SmallClass::all().count();
        assert_eq!(MASKS.iter().filter(|&&m| m != 0).count(), small);
        assert_eq!(MASKS[0], 0);
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
            assert_eq!(SLABS.class_of_word(word), Some(class));
            let entry = core::ptr::from_ref(&SLABS.masks[class.class().index() as usize]).addr();
            assert_eq!(entry, word + MASK_DISP);
            // Not at a slab word's page offset.
            assert!(entry % SLAB_BYTES >= Slab::USED_BYTES);
        }
        assert_eq!(SLABS.class_of_word(base), None);
        assert_eq!(SLABS.class_of_word(base + 4), None);
        assert_eq!(SLABS.class_of_word(base + 8 * (SMALL_COUNT + 1)), None);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn publishing_writes_every_small_mask_once_and_nothing_else() {
        publish_masks();
        publish_masks();
        for (i, entry) in SLABS.masks.iter().enumerate() {
            assert_eq!(entry.load(Ordering::Relaxed), MASKS[i] as u64);
        }
    }
}
