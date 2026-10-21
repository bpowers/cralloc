//! How long one step of asmalloc's maintenance takes (`Asmalloc::maintain_step`), the unit a
//! tokio task yields between: after a burst that leaves a few hundred MiB free across several
//! threads' caches, rounds of steps every half second, timed one step at a time.
//!
//! usage: maintain-steps [busy]    with `busy`, the burst's threads keep doing light work in
//!                                 their caches while the rounds run, so that each purge's
//!                                 madvise shoots down their TLBs too
//!
//! Prints, per round, the steps it took, its total time and its longest step, then the
//! distribution of every step's time. With `STEP_LOG` set, also each step over 50 us with its
//! wall-clock start, to line up with a trace of the madvise and membarrier calls.

use asmalloc::Asmalloc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

#[global_allocator]
static GLOBAL: Asmalloc = Asmalloc;

const THREADS: usize = 4;
const SIZES: [usize; 12] = [16, 24, 32, 40, 48, 64, 80, 96, 128, 160, 192, 256];

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

fn main() {
    let busy = std::env::args().nth(1).as_deref() == Some("busy");
    // Each step over 50 us with its wall-clock start, to line up with a trace of system calls.
    let log = std::env::var_os("STEP_LOG").is_some();
    let stop = Arc::new(AtomicBool::new(false));
    let freed = Arc::new(Barrier::new(THREADS + 1));
    let workers: Vec<_> = (0..THREADS)
        .map(|t| {
            let (stop, freed) = (stop.clone(), freed.clone());
            std::thread::spawn(move || {
                let mut s = 7 + t as u64;
                // About 50 MiB each, freed in random order.
                let mut held: Vec<Vec<u8>> =
                    (0..400_000).map(|_| vec![1u8; SIZES[next(&mut s) as usize % 12]]).collect();
                for i in (1..held.len()).rev() {
                    held.swap(i, next(&mut s) as usize % (i + 1));
                }
                drop(held);
                freed.wait();
                while !stop.load(Relaxed) {
                    if busy {
                        for _ in 0..1000 {
                            drop(std::hint::black_box(vec![0u8; SIZES[next(&mut s) as usize % 12]]));
                        }
                    } else {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            })
        })
        .collect();
    freed.wait();
    let mut steps = Vec::new();
    for round in 0..16 {
        std::thread::sleep(Asmalloc::MAINTAIN_EVERY);
        let start = Instant::now();
        let (mut count, mut longest) = (0, Duration::ZERO);
        loop {
            let wall = std::time::SystemTime::now();
            let before = Instant::now();
            let more = Asmalloc::maintain_step();
            let took = before.elapsed();
            if log && took > Duration::from_micros(50) {
                let at = wall.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
                println!("long step {count} at {at:.6} took {:.1} us", took.as_secs_f64() * 1e6);
            }
            count += 1;
            longest = longest.max(took);
            steps.push(took);
            if !more {
                break;
            }
        }
        println!(
            "round {round:2}: {count:5} steps, {:8.1} us, longest {:6.1} us",
            start.elapsed().as_secs_f64() * 1e6,
            longest.as_secs_f64() * 1e6
        );
    }
    stop.store(true, Relaxed);
    for worker in workers {
        worker.join().unwrap();
    }
    steps.sort();
    let at = |q: f64| steps[((steps.len() - 1) as f64 * q) as usize].as_secs_f64() * 1e6;
    println!(
        "steps {}: p50 {:.2} us, p99 {:.1} us, p99.9 {:.1} us, max {:.1} us",
        steps.len(),
        at(0.5),
        at(0.99),
        at(0.999),
        at(1.0)
    );
}
