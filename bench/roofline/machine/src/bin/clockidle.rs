//! The clock a CPU runs at, and what idle costs the first work after it.
//!
//! ```text
//! clockidle ramp CPU [window_ms] [windows]   cycles over wall time from the first instant after idle
//! clockidle load CPU,CPU...                  the steady clock on each of these busy at once
//! clockidle wake SERVER CLIENT               a futex wake after an idle gap: latency, deep-idle
//!                                            entries, and the time of the first work after it
//! ```
//!
//! A thread that never sleeps has cycles over wall time equal to its clock. Reading the counter is
//! a system call, a small share of a window of a few milliseconds.
use roofline_machine::*;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::{Acquire, Relaxed, Release}};
use std::time::Duration;

fn ramp(cpu: usize, window_ms: u64, windows: usize) {
    pin(cpu);
    let cycles = Counter::cycles(cpu);
    let window = window_ms * tick_rate() / 1000;
    println!("# CPU {cpu}: GHz in windows of {window_ms} ms from the first instant after each pause");
    for pause in [1u64, 10, 100, 1000, 5000, 1000, 100, 10, 1] {
        std::thread::sleep(Duration::from_millis(pause));
        let mut row = Vec::with_capacity(windows);
        let (mut t0, mut c0) = (ticks(), cycles.read());
        for _ in 0..windows {
            let end = t0 + window;
            while ticks() < end {}
            let (t1, c1) = (ticks(), cycles.read());
            row.push((c1 - c0) as f64 / ns_of(t1 - t0));
            (t0, c0) = (t1, c1);
        }
        let text: Vec<String> = row.iter().map(|g| format!("{g:.3}")).collect();
        println!("pause {pause:5} ms: {}", text.join(" "));
    }
}

fn load(cpus: &[usize]) {
    const MS: u64 = 500;
    let barrier = Barrier::new(cpus.len());
    let results: Vec<(usize, f64)> = std::thread::scope(|s| {
        let hs: Vec<_> = cpus
            .iter()
            .map(|&cpu| {
                let barrier = &barrier;
                s.spawn(move || {
                    pin(cpu);
                    let cycles = Counter::cycles(cpu);
                    barrier.wait();
                    let (t0, c0) = (ticks(), cycles.read());
                    let limit = t0 + MS * tick_rate() / 1000;
                    while ticks() < limit {}
                    let (t1, c1) = (ticks(), cycles.read());
                    (cpu, (c1 - c0) as f64 / ns_of(t1 - t0))
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let text: Vec<String> = results.iter().map(|(c, g)| format!("{c}:{g:.3}")).collect();
    println!("{} busy: {} GHz", cpus.len(), text.join(" "));
}

/// How many times this CPU has entered its deepest idle state, and that state's name.
fn deep_entries(cpu: usize) -> (String, u64) {
    let dir = format!("/sys/devices/system/cpu/cpu{cpu}/cpuidle");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return ("none".into(), 0);
    };
    let deepest = read.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("state")).map(|e| e.path()).max();
    let Some(state) = deepest else {
        return ("none".into(), 0);
    };
    let name = std::fs::read_to_string(state.join("name")).unwrap_or_default().trim().to_string();
    let usage = std::fs::read_to_string(state.join("usage")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    (name, usage)
}

fn futex_wait(word: &AtomicU32, value: u32) {
    const FUTEX_WAIT_PRIVATE: i64 = 128;
    // SAFETY: a live aligned word and no timeout.
    unsafe { syscall(SYS_FUTEX, word.as_ptr(), FUTEX_WAIT_PRIVATE, value as i64, 0i64, 0i64, 0i64) };
}

fn futex_wake(word: &AtomicU32) {
    const FUTEX_WAKE_PRIVATE: i64 = 1 | 128;
    // SAFETY: a live aligned word.
    unsafe { syscall(SYS_FUTEX, word.as_ptr(), FUTEX_WAKE_PRIVATE, 1i64, 0i64, 0i64, 0i64) };
}

/// The chase a request's first work stands for, and what it costs after each kind of wait.
fn wake(server: usize, client: usize) {
    let line = line(server);
    let sets: [(&str, usize); 3] = [("64 KiB", 64 << 10), ("512 KiB", 512 << 10), ("4 MiB", 4 << 20)];
    let chases: Vec<(usize, usize)> = sets
        .iter()
        .map(|&(_, size)| {
            let map = Box::leak(Box::new(Map::resident(size.max(huge()), Pages::Huge)));
            let mut at: Vec<*mut u8> = (0..size / line).map(|i| map.p.wrapping_add(i * line)).collect();
            Rng(0x600d_f00d).shuffle(&mut at);
            (link(&at) as usize, size / line)
        })
        .collect();
    let word = AtomicU32::new(0);
    let posted = AtomicU64::new(0);
    let woke = AtomicU64::new(0);
    let worked = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
    let done = AtomicU32::new(0);
    let quit = AtomicBool::new(false);
    println!("# server on CPU {server}, client on {client}: a request every gap");
    println!("# gap_us  wake_ns_p50  wake_ns_p90  waker_call_ns_p50  deep_entries_a_request  first_work_ns_a_load p50: 64KiB 512KiB 4MiB");
    let (word, posted, woke, worked, done, quit, chases) = (&word, &posted, &woke, &worked, &done, &quit, &chases);
    std::thread::scope(|s| {
        s.spawn(move || {
            pin(server);
            let mut seen = 0u32;
            loop {
                while word.load(Acquire) == seen {
                    if quit.load(Relaxed) {
                        return;
                    }
                    futex_wait(word, seen);
                }
                seen = word.load(Acquire);
                woke.store(ticks(), Release);
                for (i, &(start, lines)) in chases.iter().enumerate() {
                    let t0 = ticks();
                    std::hint::black_box(chase(start as *const usize, lines));
                    worked[i].store(ticks() - t0, Relaxed);
                }
                done.store(seen, Release);
            }
        });
        pin(client);
        for gap_us in [0u64, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10_000, 20_000, 50_000, 100_000] {
            let reps = if gap_us >= 20_000 { 40 } else if gap_us >= 1000 { 100 } else { 300 };
            let (mut wakes, mut calls, mut a, mut b, mut c) = (vec![], vec![], vec![], vec![], vec![]);
            let (name, before) = deep_entries(server);
            let gap = gap_us * tick_rate() / 1_000_000;
            let mut last = 0u32;
            for _ in 0..reps {
                let end = ticks() + gap;
                // The client spins, as a load generator does, so only the server sleeps.
                while ticks() < end {}
                last += 1;
                let t0 = ticks();
                posted.store(t0, Relaxed);
                word.store(last, Release);
                futex_wake(word);
                calls.push(ns_of(ticks() - t0));
                while done.load(Acquire) != last {}
                wakes.push(ns_of(woke.load(Acquire).saturating_sub(t0)));
                a.push(ns_of(worked[0].load(Relaxed)) / chases[0].1 as f64);
                b.push(ns_of(worked[1].load(Relaxed)) / chases[1].1 as f64);
                c.push(ns_of(worked[2].load(Relaxed)) / chases[2].1 as f64);
            }
            let (_, after) = deep_entries(server);
            wakes.sort_by(|x, y| x.partial_cmp(y).unwrap());
            let p90 = wakes[wakes.len() * 9 / 10];
            println!(
                "{gap_us:7} {:8.0} {p90:8.0} {:8.0} {:6.2} ({name})  {:7.2} {:7.2} {:7.2}",
                median(&mut wakes),
                median(&mut calls),
                (after - before) as f64 / reps as f64,
                median(&mut a),
                median(&mut b),
                median(&mut c),
            );
            word.store(0, Release);
            done.store(0, Release);
            // Let the server see the reset: it sleeps on the new value.
            futex_wake(word);
            std::thread::sleep(Duration::from_millis(1));
            word.store(0, Release);
        }
        quit.store(true, Relaxed);
        word.store(u32::MAX, Release);
        futex_wake(word);
    });
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let num = |i: usize, d: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    match args.get(1).map(String::as_str) {
        Some("ramp") => ramp(num(2, 8), num(3, 2) as u64, num(4, 40)),
        Some("load") => load(&parse_cpus(args.get(2).map_or("8", String::as_str))),
        Some("wake") => wake(num(2, 8), num(3, 9)),
        _ => eprintln!("usage: clockidle ramp|load|wake ..."),
    }
}
