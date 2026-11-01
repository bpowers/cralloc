//! Knobs shared by the integration tests.
#![allow(dead_code)]

use cralloc_testkit::{Profile, Stats};

/// Operations for a run: `native` times `CRALLOC_TESTKIT_SCALE` (default 1), or `miri` under
/// Miri, which runs a few thousand times slower.
pub fn ops(native: usize, miri: usize) -> usize {
    if cfg!(miri) {
        return miri;
    }
    let scale = std::env::var("CRALLOC_TESTKIT_SCALE")
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

/// Runs `body` while another thread drains the simulated heap's slabs: it asks for a drain of
/// each slab in turn, whatever the slab is doing, and moves the heap's clock so that the idle
/// check drains too. The clock stands still unless a test moves it and the idle check finds few
/// slabs quiet, so without this a run on the simulated heap sees no drain at all, and drains
/// racing the threads that use a slab are what such a run is for.
pub fn with_drains<T>(heap: &cralloc::SimHeap, body: impl FnOnce() -> T) -> T {
    use std::sync::atomic::{AtomicBool, Ordering};
    /// Stops the drainer when the body ends, by a panic too, or the scope would never end.
    struct Stop<'a>(&'a AtomicBool);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut cpu = 0;
            while !stop.load(Ordering::Acquire) {
                heap.os().advance_clock(cralloc::depot::IDLE_TICKS / 4);
                heap.drain(cpu);
                cpu = (cpu + 1) % cralloc::os::sim::SIM_THREADS;
                // Miri runs one thread at a time and a drain is some hundred atomic
                // instructions, so there the drainer mostly yields.
                if cfg!(miri) {
                    for _ in 0..2000 {
                        std::thread::yield_now();
                    }
                } else {
                    for _ in 0..200 {
                        std::hint::spin_loop();
                    }
                }
            }
        });
        let _stop = Stop(&stop);
        body()
    })
}
