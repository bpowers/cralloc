//! Sizes for tests that Miri runs: a test that repeats a step thousands of times, or builds an
//! input of megabytes, for coverage natively asks here for its count, and under Miri, which
//! interprets each step at about a thousandth of native speed, gets one that walks the same
//! paths in seconds (`scripts/miri` limits each test's CPU time).

/// `native`, or `miri` in a build for Miri.
pub(crate) const fn under_miri(native: usize, miri: usize) -> usize {
    if cfg!(miri) { miri } else { native }
}
