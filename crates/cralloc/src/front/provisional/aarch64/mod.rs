//! The routines for aarch64 that are not inlined.

#[cfg(target_env = "musl")]
pub(crate) mod area;

pub(crate) mod centry;

pub(crate) mod exports;

pub(crate) mod selftest;

pub(crate) mod slab;

// What the routines' text writes as numbers is the specification's. One `adrp` of the page at
// `FRONT_PAGE` from `SLABS` reaches the published words and the masks in its own page and the
// region table in the next, and `SLABS` is page-aligned, so none of them needs `:lo12:`.
const _: () = {
    use crate::spec;
    const FRONT_PAGE: usize = 33554432;
    assert!(align_of::<crate::front::Front>() >= 4096 && FRONT_PAGE.is_multiple_of(4096));
    assert!(spec::SHIFT_DISP == FRONT_PAGE + 1024 && spec::BASE_DISP == spec::SHIFT_DISP + 8);
    assert!(spec::MASK_DISP == FRONT_PAGE + 2048);
    assert!(spec::REGION_DISP == spec::MASK_DISP + 4096 && spec::REGION_COUNT == 4096);
    assert!(spec::TABLE_OFF == 2048 && spec::SIZED_LEN == 0x10000);
    assert!(spec::MALLOC_MAX == 8 << 12 && spec::CALLOC_MAX == 1024);
    assert!(spec::SLAB_COUNT == 2 << 12 && spec::SLAB_SHIFT == 12);
    assert!(spec::NO_CPU == 2 && spec::RSEQ_CPU_ID == 4 && spec::RSEQ_CS == 8);
};
