//! cralloc's back-end fill and launder with several threads on one class at once: what a
//! program whose threads all refill the same size class pays.
//!
//!   backcontend run [threads=8] [class=16] [every=1] [batch=1022] [rounds=2000] [pool=65536]
//!
//! The benchmark keeps a heap of its own (a `back::State` over the real OS). It carves `pool`
//! objects of the class and launders one in `every` of them (`every=1`: all of them, so the
//! words are dense; `every=8`: one free slot in 8, a fragmented heap), holding the rest. Then
//! `threads` threads, thread `i` pinned to the `i`-th CPU of `BENCH_CPUS` (wrapping, so twice
//! as many threads as CPUs puts two on each), each repeat `rounds` times: fill a magazine's
//! worth (`batch`, 1022 being a 16-byte class's magazine) and launder it again in the order a
//! magazine is popped (last filled first). Every thread starts together and works on the same
//! free slots, lowest address first, as fillers of one class do.
//!
//! The free slots outnumber what all threads can hold at once, so no fill carves. Each thread
//! counts its own user-mode cycles and instructions over its whole loop; the time-stamp counter
//! splits the cycles between fill and launder, since enabling and reading counters around each
//! call would cost more than the call. A line reports the sums over all threads divided by the
//! objects moved: the cost per object to each thread, comparable with the one-thread figure.
//! The wall time per object per thread is there too. Run it under `scripts/memlimit`.

use bench_harness::perf::{Event, Target};
use bench_harness::{Args, Counters, Cpus};
use cralloc::back::State;
use cralloc::classes::Class;
use cralloc::os::linux::LinuxOs;
use std::sync::Barrier;
use std::time::Instant;

static STATE: State<LinuxOs> = State::new(LinuxOs);

/// One thread's counts over its rounds.
#[derive(Clone, Copy, Default)]
struct Tally {
    cycles: u64,
    instructions: u64,
    fill_ticks: u64,
    launder_ticks: u64,
    objects: u64,
    wall_ns: u64,
}

fn ticks() -> u64 {
    // SAFETY: rdtsc has no preconditions on x86-64.
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn main() {
    let (command, args) = Args::from_env();
    assert_eq!(command, "run", "usage: backcontend run [key=value ...]");
    let threads = args.usize("threads", 8);
    let size = args.usize("class", 16);
    let every = args.usize("every", 1);
    let batch = args.usize("batch", 1022);
    let rounds = args.usize("rounds", 2000);
    let pool = args.usize("pool", 65536);
    let cpus = Cpus::from_env();
    let class = Class::of_size(size).and_then(Class::small).expect("a small class size");
    assert!(pool / every >= 2 * threads * batch, "too few free slots: fills would carve");

    let back = STATE.get_or_reserve().expect("reserve the heap");
    let mut held = vec![std::ptr::null_mut::<u8>(); pool];
    let mut got = 0;
    while got < pool {
        let n = back.fill(class, &mut held[got..(got + 256).min(pool)]);
        assert!(n > 0, "the back end ran out of memory");
        got += n;
    }
    held.sort();
    let freed: Vec<*mut u8> = held.iter().step_by(every).copied().collect();
    for chunk in freed.chunks(64) {
        // SAFETY: the bench filled these slots itself and uses none of them.
        unsafe { back.launder(class, chunk) };
    }

    let start = Barrier::new(threads);
    let tallies: Vec<Tally> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|me| {
                let (start, cpus) = (&start, &cpus);
                scope.spawn(move || {
                    cpus.pin(me);
                    let back = STATE.get_or_reserve().expect("the heap");
                    // Opened by this thread, which starts no other: they count it alone.
                    let counters =
                        Counters::open(&[Event::CyclesUser, Event::Instructions], Target::Me).expect("perf_event_open");
                    let mut magazine = vec![std::ptr::null_mut::<u8>(); batch];
                    let mut tally = Tally::default();
                    start.wait();
                    let t0 = Instant::now();
                    let before = counters.read();
                    counters.enable();
                    for _ in 0..rounds {
                        let a = ticks();
                        assert_eq!(back.fill(class, &mut magazine[..]), batch, "a fill came up short");
                        let b = ticks();
                        magazine.reverse();
                        // SAFETY: this thread was just given these slots and uses none of them.
                        unsafe { back.launder(class, &magazine) };
                        let c = ticks();
                        tally.fill_ticks += b - a;
                        tally.launder_ticks += c - b;
                        tally.objects += batch as u64;
                    }
                    counters.disable();
                    tally.wall_ns = t0.elapsed().as_nanos() as u64;
                    let counted = counters.read().since(&before);
                    assert!(!counted.multiplexed, "counters were multiplexed");
                    tally.cycles = counted.get("cycles_user").unwrap_or(0);
                    tally.instructions = counted.get("instructions").unwrap_or(0);
                    tally
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("a bench thread panicked")).collect()
    });

    let sum = |f: fn(&Tally) -> u64| tallies.iter().map(f).sum::<u64>() as f64;
    let objects = sum(|t| t.objects);
    let cycles = sum(|t| t.cycles) / objects;
    let fill_share = sum(|t| t.fill_ticks) / (sum(|t| t.fill_ticks) + sum(|t| t.launder_ticks));
    let worst = tallies.iter().map(|t| t.cycles as f64 / t.objects as f64).fold(0.0, f64::max);
    println!(
        "threads={threads:>2} cpus={} class={} every={every} batch={batch} rounds={rounds}: \
         {:6.1} cyc/obj (fill {:6.1}, launder {:5.1}; worst thread {:6.1}), {:5.1} ins/obj, \
         wall {:6.1} ns/obj/thread",
        cpus.to_list(),
        class.size(),
        cycles,
        cycles * fill_share,
        cycles * (1.0 - fill_share),
        worst,
        sum(|t| t.instructions) / objects,
        sum(|t| t.wall_ns) / objects,
    );
    drop(held);
}
