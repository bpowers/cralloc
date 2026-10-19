//! What the thread that starts a clocked pass pays to rotate every region's frontier
//! (`back::purge`, `rotate_frontiers`), warm and with the caches evicted first.
//!
//!   backrotate run [small=40] [large=20] [reps=31] [evict_mb=256] [rise=1]
//!
//! The benchmark keeps a heap of its own (a `back::State` over the real OS) and gives `small`
//! small classes and `large` large classes a region each by allocating from them. Nothing is
//! ever freed, so nothing is retained, and a heartbeat an interval after the last pass takes the
//! path that ends the pass as it starts: a clock read, the tallies, one compare-and-swap of the
//! decay word, and the rotation. With `rise=1` every small class carves again before each
//! heartbeat, so every small region's word is one the rotation must write. The figure is
//! time-stamp ticks for that one heartbeat: the median and the range over `reps`, first with
//! nothing done in between (warm), then after writing `evict_mb` MiB of other memory (cold).
//! A heartbeat with no pass due, measured the same way, is the baseline to subtract.
//!
//! Pin it with `BENCH_CPUS` and run it under `scripts/memlimit` and `scripts/measure`.

use asmalloc::back::{Back, State};
use asmalloc::classes::{Class, SmallClass};
use asmalloc::os::linux::LinuxOs;
use bench_harness::{Args, Cpus};
use std::cell::Cell;

static STATE: State<LinuxOs> = State::new(LinuxOs);

/// `policy::DECAY_TICKS`, which the crate does not export.
const INTERVAL: u64 = 1 << 32;

fn ticks() -> u64 {
    // SAFETY: rdtsc has no preconditions on x86-64.
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn carve(back: &Back<'_, LinuxOs>, classes: &[SmallClass]) {
    for &class in classes {
        let mut out = [core::ptr::null_mut::<u8>(); 8];
        assert_eq!(back.fill(class, &mut out[..]), 8, "the back end ran out of memory");
    }
}

fn summary(mut v: Vec<u64>) -> (u64, u64, u64) {
    v.sort_unstable();
    (v[0], v[v.len() / 2], v[v.len() - 1])
}

fn main() {
    let (command, args) = Args::from_env();
    assert_eq!(command, "run", "usage: backrotate run [key=value ...]");
    Cpus::from_env().pin(0);
    let small = args.usize("small", 40);
    let large = args.usize("large", 20);
    let reps = args.usize("reps", 31);
    let evict = args.usize("evict_mb", 256) << 20;
    let rise = args.usize("rise", 1) != 0;
    let back = STATE.get_or_reserve().expect("reserve the heap");
    let smalls: Vec<SmallClass> = SmallClass::all().take(small).collect();
    carve(&back, &smalls);
    let larges: Vec<Class> = Class::all().filter(|c| !c.is_small()).take(large).collect();
    for &class in &larges {
        back.alloc(class, class.size()).expect("a large block");
    }
    assert_eq!(back.retained(), 0);
    let mut junk = vec![1u8; evict.max(1)];
    let clock = Cell::new(INTERVAL);
    let now = || clock.get();
    // The first heartbeat stamps the decay word.
    back.heartbeat_with(&now);
    for cold in [false, true] {
        let (mut due, mut idle) = (Vec::new(), Vec::new());
        for rep in 0..reps {
            if rise {
                carve(&back, &smalls);
            }
            for due_now in [true, false] {
                if due_now {
                    clock.set(clock.get() + INTERVAL);
                }
                if cold {
                    for (i, byte) in junk.iter_mut().enumerate().step_by(64) {
                        *byte = byte.wrapping_add(i as u8 ^ rep as u8);
                    }
                }
                let start = ticks();
                back.heartbeat_with(&now);
                let took = ticks() - start;
                if due_now { &mut due } else { &mut idle }.push(took);
            }
        }
        let (dl, dm, dh) = summary(due);
        let (il, im, ih) = summary(idle);
        println!(
            "{} small={small} large={large} rise={}: pass due {dm} ticks [{dl}, {dh}]; none due {im} [{il}, {ih}]",
            if cold { "cold" } else { "warm" },
            rise as u8,
        );
    }
    std::hint::black_box(&mut junk);
}
