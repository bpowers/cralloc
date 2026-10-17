//! The front end: per-CPU slabs of magazines, and the routines that work on them.
//!
//! Each CPU number has a [`Slab`] in the static [`SLABS`]. A slab's hot word for a small class
//! is the address of the next free slot of the magazine loaded for that class, or zero, or a
//! marker; its cold part holds the CPU's spare magazines and the counters the idle check and
//! the drain use. The array is zero-initialized, and zero is a valid slab with no magazines,
//! so nothing needs initializing before the first call.
//!
//! Only the [`Routines`] write a slab, each with one restartable sequence that commits with a
//! single store, and the drain (`depot::drain`). The routines are assembly generated from Lean;
//! [`Twin`] implements the same specifications in Rust for every environment where assembly
//! cannot run, over a simulated processor ([`Machine`](crate::os::Machine)) that tests control.
//!
//! The layout constants here must equal those the Lean specification gives the generated code
//! (`lean/generated/*/slab.rs`); a test compares them.

#[cfg(test)]
pub(crate) mod check;
mod routines;
mod slab;
pub(crate) mod sync;
mod twin;
#[cfg(kani)]
mod verify;

pub use routines::{NO_CPU, Routines, StackPop, StackPush, StoreIfEqual};
pub use slab::{Cpu, Field, SLAB_BYTES, SLAB_COUNT, SLAB_SHIFT, Slab, Slabs, Spare};
pub use twin::Twin;

use crate::classes::{MagazineSize, SMALL_COUNT, SmallClass};

/// The slabs, one per CPU number. Zero-initialized: untouched slabs cost no memory.
pub static SLABS: Slabs<[Slab; SLAB_COUNT]> = Slabs::new();

/// The slot mask of each small class's magazines, indexed by class index, for the out-of-line
/// routines, which take the class in a register. Index 0 is no class; it holds the smallest
/// magazine's mask so that every entry is a valid mask.
pub static MAG_MASKS: [u32; SMALL_COUNT + 1] = {
    let mut masks = [MagazineSize::MIN.mask() as u32; SMALL_COUNT + 1];
    let mut ordinal = 0;
    while ordinal < SMALL_COUNT {
        if let Some(class) = SmallClass::from_ordinal(ordinal) {
            masks[class.class().index() as usize] = class.magazine().mask() as u32;
        }
        ordinal += 1;
    }
    masks
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::{MARKER_SHIFT, SLOT_BYTES};
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
            include_str!("../../../../lean/generated/x86_64/slab.rs"),
            include_str!("../../../../lean/generated/x86_64_inline/slab.rs"),
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
    fn the_mask_table_holds_each_class_mask_at_its_index() {
        for class in SmallClass::all() {
            assert_eq!(
                MAG_MASKS[class.class().index() as usize] as usize,
                class.magazine().mask()
            );
        }
        assert_eq!(MAG_MASKS.len(), SMALL_COUNT + 1);
    }

    #[test]
    fn the_static_slabs_are_one_per_cpu_number() {
        let slabs: &Slabs = &SLABS;
        assert_eq!(slabs.len(), SLAB_COUNT);
        assert_eq!(size_of_val(&SLABS), SLAB_COUNT << SLAB_SHIFT);
        assert_eq!(core::ptr::from_ref(&SLABS).addr() % SLAB_BYTES, 0);
    }
}
