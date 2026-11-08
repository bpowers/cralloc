//! Fast-path roofline on an aarch64 core: hand-written loops timed by user-mode counters on the
//! PMU of the CPU they run on, found in sysfs. Written for Apple Firestorm, and the same command
//! runs on any aarch64 Linux.
//!
//!   roofline-aarch64 [PATTERN[,PATTERN...]]   run kernels whose names start with a pattern
//!   roofline-aarch64 list [PATTERN,...]       list them
//!   roofline-aarch64 clock                    is a cycle a fixed time, from cold and warm?
//!   roofline-aarch64 offsets [SIDE [ITERS]]   page-offset aliasing over fresh pages
//!
//! A pattern ending in `$` must match the whole name. `RF_CPU` (the last CPU but one), `RF_ITERS`
//! (4000000), `RF_REPS` (4, the first a warm-up), `RF_ROUNDS` (3: every kernel once a round,
//! interleaved), `RF_EVENTS` (comma-separated: `name=0xCODE`, an event in the PMU's sysfs
//! directory, or a name in this PMU's table below), `RF_PMU` (overrides the search).
//!
//! A cell is the fastest repetition over all rounds; `+x` is how far the slowest round's fastest
//! repetition sits above it.
mod alias;
mod env;
mod perf;
mod tables;

use env::{Env, Kernel, Kind};
use perf::{Counter, Pmu};
use std::time::Instant;

std::arch::global_asm!(include_str!(concat!(env!("OUT_DIR"), "/kernels.s")), options(raw));

pub struct Spec {
    pub name: &'static str,
    pub f: Kernel,
    pub kind: Kind,
    pub ops: u64,
    pub note: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/specs.rs"));

/// Raw events by PMU. Linux's `apple_m1_cpu_pmu.c` has the names, shortened here;
/// `docs/research/firestorm.md` says which were validated and by what kernel.
const FIRESTORM_EVENTS: &[(&str, u64)] = &[
    ("uops", 0x01),
    ("sched", 0x52),
    ("map_stall_disp", 0x70),
    ("rewind", 0x75),
    ("map_stall", 0x76),
    ("map_int", 0x7c),
    ("map_ldst", 0x7d),
    ("flush", 0x84),
    ("branch", 0x8d),
    ("taken", 0x90),
    ("cond", 0x94),
    ("int_ld", 0x95),
    ("int_st", 0x96),
    ("int_alu", 0x97),
    ("ldst", 0x9b),
    ("barrier", 0x9c),
    ("l1d_miss_st", 0xa2),
    ("l1d_miss_ld", 0xa3),
    ("ld_uop", 0xa6),
    ("st_uop", 0xa7),
    ("atomic_ok", 0xb3),
    ("atomic_fail", 0xb4),
    ("order_viol", 0xc4),
    ("br_miss", 0xcb),
    ("bubble", 0xd6),
    ("l1i_miss", 0xdb),
    ("fetch_restart", 0xde),
];

fn pmu_table(pmu: &Pmu) -> &'static [(&'static str, u64)] {
    if pmu.name == "apple_firestorm_pmu" { FIRESTORM_EVENTS } else { &[] }
}

/// An event named on the command line: `name=0xCODE`, the PMU's own sysfs name, or this file's
/// table for the PMU.
fn event_code(pmu: &Pmu, spec: &str) -> Option<(String, u64)> {
    if let Some((name, code)) = spec.split_once('=') {
        let code = u64::from_str_radix(code.trim_start_matches("0x"), 16).expect("a hex event");
        return Some((name.to_string(), code));
    }
    if let Some(code) = perf::sysfs_event(&pmu.name, spec) {
        return Some((spec.to_string(), code));
    }
    pmu_table(pmu).iter().find(|(n, _)| *n == spec).map(|(n, c)| (n.to_string(), *c))
}

fn envu(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

/// `RF_CPU`, or the last CPU but one: a default that is a performance core here and an ordinary
/// one on a homogeneous machine, and not the CPU interrupts favor.
fn default_cpu() -> usize {
    let online = std::fs::read_to_string("/sys/devices/system/cpu/online").unwrap_or_default();
    let last = online
        .trim()
        .rsplit([',', '-'])
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1);
    envu("RF_CPU", last.saturating_sub(1) as u64) as usize
}

fn selected(filter: &str, name: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    filter.split(',').any(|f| match f.strip_suffix('$') {
        Some(whole) => name == whole,
        None => name.starts_with(f),
    })
}

struct Cell {
    best: Vec<f64>,
    round_best: Vec<f64>,
    ghz: f64,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cpu = default_cpu();
    env::pin(cpu);
    match args.first().map(String::as_str) {
        Some("list") => {
            let filter = args.get(1).cloned().unwrap_or_default();
            for s in SPECS.iter().filter(|s| selected(&filter, s.name)) {
                println!("{:<26} {}", s.name, s.note);
            }
            return;
        }
        Some("clock") => return clock(cpu),
        Some("offsets") => return alias::main(cpu, &args[1..]),
        _ => {}
    }
    let filter = args.first().cloned().unwrap_or_default();
    let iters = envu("RF_ITERS", 4_000_000);
    let reps = envu("RF_REPS", 4) as usize;
    let rounds = envu("RF_ROUNDS", 3) as usize;
    let pmu = Pmu::of_cpu(cpu);
    let mut names = vec!["cycles".to_string(), "inst".to_string()];
    let mut codes = vec![pmu.cycles, pmu.instructions];
    let default_events = if pmu_table(&pmu).is_empty() { "" } else { "uops" };
    let events_spec = std::env::var("RF_EVENTS").unwrap_or_else(|_| default_events.to_string());
    for spec in events_spec.split(',').filter(|s| !s.is_empty()) {
        match event_code(&pmu, spec) {
            Some((n, c)) => {
                names.push(n);
                codes.push(c);
            }
            None => eprintln!("  ({spec} is not an event of {}; left out)", pmu.name),
        }
    }
    let counters: Vec<Counter> = codes
        .iter()
        .map(|&c| Counter::open(pmu.typ, c).unwrap_or_else(|| panic!("perf_event_open {c:#x}")))
        .collect();
    let env = Env::new(cpu);
    println!("{} on {}", env.describe(), pmu.name);
    let chosen: Vec<&Spec> = SPECS.iter().filter(|s| selected(&filter, s.name)).collect();
    let mut cells: Vec<Option<Cell>> = chosen.iter().map(|_| None).collect();
    let mut aborts_total = 0;
    for _ in 0..rounds {
        for (i, s) in chosen.iter().enumerate() {
            let n = (iters / s.ops).max(1);
            let mut round_best = vec![f64::MAX; codes.len()];
            let mut round_ghz = 0.0;
            for rep in 0..reps {
                let (state, aux, arr, class) = env.prepare(s.kind);
                let aborts0 = env.aborts();
                for c in &counters {
                    c.start();
                }
                let t = Instant::now();
                unsafe { (s.f)(n, state, aux, arr, class) };
                let ns = t.elapsed().as_nanos() as f64;
                let counts: Vec<Option<u64>> = counters.iter().map(Counter::stop).collect();
                aborts_total += env.aborts() - aborts0;
                if rep == 0 {
                    continue;
                }
                if counts.iter().any(Option::is_none) {
                    eprintln!("  (multiplexed repetition of {} dropped)", s.name);
                    continue;
                }
                let d = (n * s.ops) as f64;
                let c0 = counts[0].unwrap() as f64 / d;
                if c0 < round_best[0] {
                    round_best = counts.iter().map(|c| c.unwrap() as f64 / d).collect();
                    round_ghz = counts[0].unwrap() as f64 / ns;
                }
            }
            let cell = cells[i].get_or_insert(Cell {
                best: vec![f64::MAX; codes.len()],
                round_best: vec![],
                ghz: 0.0,
            });
            if round_best[0] < cell.best[0] {
                cell.best = round_best.clone();
                cell.ghz = round_ghz;
            }
            cell.round_best.push(round_best[0]);
        }
    }
    print!("{:<26} {:>8} {:>6}", "kernel", "cyc/op", "+x");
    for n in &names[1..] {
        print!(" {n:>8}");
    }
    println!(" {:>5}  what", "GHz");
    for (s, cell) in chosen.iter().zip(&cells) {
        let cell = cell.as_ref().unwrap();
        let worst = cell.round_best.iter().cloned().fold(f64::MIN, f64::max);
        if cell.best[0] == f64::MAX {
            println!("{:<26} every repetition multiplexed: name fewer events", s.name);
            continue;
        }
        print!("{:<26} {:>8.3} {:>6.3}", s.name, cell.best[0], worst - cell.best[0]);
        for v in &cell.best[1..] {
            print!(" {v:>8.2}");
        }
        println!(" {:>5.2}  {}", cell.ghz, s.note);
    }
    println!("rseq aborts: {aborts_total}");
}

/// Whether cycles run at a fixed rate: a loop of one add a cycle at a few lengths, after an idle
/// pause and after running, timed by the cycle counter and by `Instant`.
fn clock(cpu: usize) {
    let pmu = Pmu::of_cpu(cpu);
    let cycles = Counter::open(pmu.typ, pmu.cycles).unwrap();
    let f = SPECS.iter().find(|s| s.name == "w_add1").expect("w_add1").f;
    let env = Env::new(cpu);
    let (state, aux, arr, class) = env.prepare(Kind::Simple);
    println!("{:>12} {:>10} {:>12} {:>10} {:>8}", "pause", "iters", "cycles/iter", "ms", "GHz");
    let timed = |n: u64| {
        cycles.start();
        let t = Instant::now();
        unsafe { f(n, state, aux, arr, class) };
        let ns = t.elapsed().as_nanos() as f64;
        (cycles.stop().unwrap() as f64, ns)
    };
    for &(pause_ms, n) in &[
        (0u64, 300_000_000u64),
        (0, 3_000_000),
        (0, 30_000),
        (100, 30_000),
        (100, 300_000),
        (100, 3_000_000),
        (100, 30_000_000),
        (1000, 30_000),
        (1000, 300_000),
        (1000, 3_000_000),
        (1000, 30_000_000),
        (0, 300_000_000),
    ] {
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(pause_ms));
            let (c, ns) = timed(n);
            println!(
                "{:>10}ms {:>10} {:>12.3} {:>10.3} {:>8.3}",
                pause_ms,
                n,
                c / n as f64,
                ns / 1e6,
                c / ns
            );
        }
    }
}
