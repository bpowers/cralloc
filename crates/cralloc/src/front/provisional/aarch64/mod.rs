//! The routines for aarch64 that are not inlined.

#[cfg(target_env = "musl")]
pub(crate) mod area;

pub(crate) mod centry;

pub(crate) mod exports;

pub(crate) mod selftest;

pub(crate) mod slab;

// The displacements written in the routines' text are the specification's.
const _: () = {
    use crate::spec;
    assert!(spec::MASK_DISP == 33556480);
    assert!(spec::SHIFT_DISP == 33555456 && spec::BASE_DISP == spec::SHIFT_DISP + 8);
    assert!(spec::REGION_DISP == 33560576 && spec::REGION_COUNT == 4096);
    assert!(spec::TABLE_OFF == 2048 && spec::SIZED_LEN == 0x10000);
    assert!(spec::MALLOC_MAX == 8 << 12 && spec::CALLOC_MAX == 1024);
    assert!(spec::SLAB_COUNT == 2 << 12 && spec::SLAB_SHIFT == 12);
    assert!(spec::NO_CPU == 2 && spec::RSEQ_CPU_ID == 4 && spec::RSEQ_CS == 8);
};
