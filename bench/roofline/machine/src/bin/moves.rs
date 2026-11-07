//! What it costs to move a line between two CPUs.
//!
//! ```text
//! moves pingpong A B [rounds]    one line handed back and forth
//! moves atomics A B [ms]         contended and uncontended ldadd and cas on one line
//! moves nextuser A B [rounds]    the extra cost to a producer when a consumer on B read and wrote
//!                                the objects first, by object size and order
//! ```
use roofline_machine::*;
use std::sync::Barrier;
use std::sync::atomic::{
    AtomicBool, AtomicU64,
    Ordering::{Acquire, Relaxed, Release},
};

fn pingpong(a: usize, b: usize, rounds: usize) {
    let map = Map::resident(1 << 16, Pages::Small);
    // SAFETY: inside a live mapping that outlives the threads, aligned.
    let word = unsafe { AtomicU64::from_ptr(map.p.add(1 << 15).cast()) };
    const N: u64 = 1 << 20;
    let barrier = Barrier::new(2);
    let mut ns = vec![];
    let mut cycles = vec![];
    for _ in 0..rounds {
        word.store(0, Relaxed);
        let (t, c) = std::thread::scope(|s| {
            let bb = &barrier;
            s.spawn(move || {
                pin(b);
                bb.wait();
                for i in 0..N {
                    while word.load(Relaxed) != 2 * i + 1 {}
                    word.store(2 * i + 2, Relaxed);
                }
            });
            let h = s.spawn(move || {
                pin(a);
                let counter = Counter::cycles(a);
                bb.wait();
                let (t0, c0) = (ticks(), counter.read());
                for i in 0..N {
                    while word.load(Relaxed) != 2 * i {}
                    word.store(2 * i + 1, Relaxed);
                }
                while word.load(Relaxed) != 2 * N {}
                let (c1, t1) = (counter.read(), ticks());
                (ns_of(t1 - t0), (c1 - c0) as f64)
            });
            h.join().unwrap()
        });
        ns.push(t / (2 * N) as f64);
        cycles.push(c / (2 * N) as f64);
    }
    let lo = ns.iter().copied().fold(f64::MAX, f64::min);
    let hi = ns.iter().copied().fold(0.0, f64::max);
    println!(
        "pingpong CPUs {a},{b}: {:.1} ns a hand-off ({lo:.1} to {hi:.1}), {:.1} cycles on A",
        median(&mut ns),
        median(&mut cycles)
    );
}

/// Operations and failures a thread made on the word until the leader's clock said `ms`.
fn hammer(word: &AtomicU64, cas: bool, stop: &AtomicBool, leader: bool, ms: u64) -> (u64, u64, u64) {
    let limit = ms * tick_rate() / 1000;
    let t0 = ticks();
    let (mut ops, mut fails) = (0u64, 0u64);
    let mut last = word.load(Relaxed);
    loop {
        for _ in 0..64 {
            if cas {
                match word.compare_exchange(last, last.wrapping_add(1), Relaxed, Relaxed) {
                    Ok(_) => {
                        ops += 1;
                        last = last.wrapping_add(1);
                    }
                    Err(now) => {
                        fails += 1;
                        last = now;
                    }
                }
            } else {
                word.fetch_add(1, Relaxed);
                ops += 1;
            }
        }
        if leader {
            if ticks() - t0 >= limit {
                stop.store(true, Relaxed);
                break;
            }
        } else if stop.load(Relaxed) {
            break;
        }
    }
    (ops, fails, ticks() - t0)
}

fn atomics(a: usize, b: usize, ms: u64) {
    let map = Map::resident(1 << 16, Pages::Small);
    // SAFETY: inside a live mapping that outlives the threads, aligned.
    let word = unsafe { AtomicU64::from_ptr(map.p.add(1 << 14).cast()) };
    // SAFETY: as above, on another line.
    let stop = unsafe { AtomicBool::from_ptr(map.p.add(1 << 15).cast()) };
    pin(a);
    for (name, cas) in [("ldadd", false), ("cas", true)] {
        stop.store(false, Relaxed);
        let (ops, _, t) = hammer(word, cas, stop, true, ms);
        println!("{name} alone on CPU {a}: {:.2} ns an operation", ns_of(t) / ops as f64);
        let mut out = vec![];
        for _ in 0..5 {
            stop.store(false, Relaxed);
            let barrier = Barrier::new(2);
            let (ra, rb) = std::thread::scope(|s| {
                let bb = &barrier;
                let hb = s.spawn(move || {
                    pin(b);
                    bb.wait();
                    hammer(word, cas, stop, false, ms)
                });
                let ha = s.spawn(move || {
                    pin(a);
                    bb.wait();
                    hammer(word, cas, stop, true, ms)
                });
                (ha.join().unwrap(), hb.join().unwrap())
            });
            let ops = (ra.0 + rb.0) as f64;
            out.push((
                ns_of(ra.2) / ops,
                ns_of(ra.2) / ra.0.max(1) as f64,
                ns_of(rb.2) / rb.0.max(1) as f64,
                (ra.1 + rb.1) as f64 / ops,
            ));
        }
        out.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        let m = out[out.len() / 2];
        println!(
            "{name} contended CPUs {a},{b}: {:.2} ns an operation overall ({:.2} to {:.2}); per thread {:.2} and {:.2}; failures an op {:.2}",
            m.0,
            out[0].0,
            out[out.len() - 1].0,
            m.1,
            m.2,
            m.3
        );
    }
}

/// Producer on A writes each object; hand-off; consumer on B (or nobody) reads and writes each;
/// hand-off; and so on. What the producer's writes cost per object is timed by the generic timer
/// around each pass, so nothing else touches the cache in between.
fn nextuser(a: usize, b: usize, rounds: usize) {
    let line = line(a);
    let map = Map::resident(1 << 21, Pages::Small);
    // SAFETY: inside a live mapping that outlives the threads, aligned.
    let flag = unsafe { AtomicU64::from_ptr(map.p.cast()) };
    let data = map.p.wrapping_add(1 << 16) as usize;
    println!("# producer on {a}, consumer on {b}: ns a pass per object, and the extra over no consumer");
    println!("# object  order     objects  lines  alone_ns  with_consumer_ns  extra_ns_per_object  extra_ns_per_line");
    for (size, count) in [(64usize, 512usize), (128, 256), (256, 128)] {
        for shuffled in [false, true] {
            let mut order: Vec<usize> = (0..count).map(|i| data + i * size).collect();
            if shuffled {
                Rng(0xfeed_beef_1234_5677).shuffle(&mut order);
            }
            let order = &order;
            let words = size / 8;
            let write = move |tag: u64| {
                for &o in order {
                    let p = o as *mut u64;
                    for w in 0..words {
                        // SAFETY: inside the mapping; the hand-off gives this thread the objects.
                        unsafe { p.add(w).write_volatile(tag) };
                    }
                }
            };
            let read_write = move |tag: u64| {
                let mut sum = 0u64;
                for &o in order {
                    let p = o as *mut u64;
                    for w in 0..words {
                        // SAFETY: as above.
                        unsafe {
                            sum = sum.wrapping_add(p.add(w).read_volatile());
                            p.add(w).write_volatile(tag);
                        }
                    }
                }
                std::hint::black_box(sum);
            };
            let passes = 4096u64;
            let measure = |consumer: bool| -> f64 {
                flag.store(0, Relaxed);
                let barrier = Barrier::new(2);
                let bb = &barrier;
                std::thread::scope(|s| {
                    s.spawn(move || {
                        pin(b);
                        bb.wait();
                        for i in 0..passes {
                            while flag.load(Acquire) != 2 * i + 1 {}
                            if consumer {
                                read_write(i);
                            }
                            flag.store(2 * i + 2, Release);
                        }
                    });
                    let h = s.spawn(move || {
                        pin(a);
                        bb.wait();
                        let mut sum = 0u64;
                        for i in 0..passes {
                            while flag.load(Acquire) != 2 * i {}
                            let t0 = ticks();
                            write(i);
                            sum += ticks() - t0;
                            flag.store(2 * i + 1, Release);
                        }
                        ns_of(sum) / passes as f64 / count as f64
                    });
                    h.join().unwrap()
                })
            };
            let (mut alone, mut with) = (vec![], vec![]);
            for _ in 0..rounds {
                alone.push(measure(false));
                with.push(measure(true));
            }
            let (x, y) = (median(&mut alone), median(&mut with));
            let lines = (count * size).div_ceil(line);
            println!(
                "{size:6}  {:9} {count:6} {lines:6}  {x:8.2}  {y:8.2}  {:8.2}  {:8.2}",
                if shuffled { "shuffled" } else { "ascending" },
                y - x,
                (y - x) * count as f64 / lines as f64
            );
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let num = |i: usize, d: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    match args.get(1).map(String::as_str) {
        Some("pingpong") => pingpong(num(2, 8), num(3, 9), num(4, 5)),
        Some("atomics") => atomics(num(2, 8), num(3, 9), num(4, 30) as u64),
        Some("nextuser") => nextuser(num(2, 8), num(3, 9), num(4, 5)),
        _ => eprintln!("usage: moves pingpong|atomics|nextuser"),
    }
}
