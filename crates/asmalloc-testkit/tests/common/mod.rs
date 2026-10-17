//! Knobs shared by the integration tests.
#![allow(dead_code)]

use asmalloc_testkit::{Profile, Stats};

/// Operations for a run: `native` times `ASMALLOC_TESTKIT_SCALE` (default 1), or `miri` under
/// Miri, which runs a few thousand times slower.
pub fn ops(native: usize, miri: usize) -> usize {
    if cfg!(miri) {
        return miri;
    }
    let scale = std::env::var("ASMALLOC_TESTKIT_SCALE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1);
    native * scale.max(1)
}

/// `p`, with sizes and alignments small enough for Miri when running under it.
pub fn fit(p: Profile) -> Profile {
    if cfg!(miri) {
        p.capped(4096, 4096, 1 << 20)
    } else {
        p
    }
}

/// Every preset, fitted.
pub fn profiles() -> Vec<Profile> {
    Profile::ALL.into_iter().map(fit).collect()
}

/// Operations for one run of `p`: large blocks cost more per operation.
pub fn ops_for(p: &Profile) -> usize {
    if p.name == Profile::LARGE_GROWING.name {
        ops(1500, 60)
    } else {
        ops(15_000, 150)
    }
}

/// Every block the run allocated was freed exactly once.
pub fn balanced(stats: &Stats, what: &str) {
    assert!(stats.ops > 0, "{what}: nothing ran");
    assert_eq!(
        stats.frees,
        stats.blocks_allocated(),
        "{what}: every block is freed exactly once: {stats:?}"
    );
}

/// Workers for multithreaded runs.
pub fn workers() -> usize {
    if cfg!(miri) { 3 } else { 6 }
}
