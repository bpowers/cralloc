//! The slab word's store-to-load chain against code placement.
//!
//!   roofline-chain FORMS [PLACES]
//!
//! FORMS is a comma-separated list of the forms `gen.py` names (`if,ifg,ia,ip,td,cf,cm,cp`, or
//! `all`); PLACES a comma-separated list of placements or ranges (`0-63`, `2048`), default every
//! placement emitted. Prints, per form and placement, cycles and instructions per operation and
//! the events named in RF_EVENTS (up to three, `name` from the table below or `name=0xCONFIG`),
//! counted in user mode on plain counters, one counter per event, no groups.
//!
//! Every item runs RF_REPS times (default 3) in each of RF_ROUNDS interleaved rounds (default
//! 3), the placements of a round in a shuffled order; a cell is the fastest repetition, and `+x`
//! how much slower the slowest round's best was. RF_OPS (default 2,000,000) operations per
//! repetition; RF_CPU (default 2) the CPU.
use std::io::Write;

include!(concat!(env!("OUT_DIR"), "/kernels.rs"));

extern "C" {
    fn syscall(num: i64, ...) -> i64;
    fn ioctl(fd: i32, req: u64, ...) -> i32;
    fn read(fd: i32, buf: *mut core::ffi::c_void, n: usize) -> isize;
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn sched_setaffinity(pid: i32, sz: usize, mask: *const u64) -> i32;
    static __rseq_offset: isize;
    static mut ck_slabs: [u8; 0];
    static mut ck_csizes: [u8; 0];
    static mut ck_aborts: u64;
    fn ck_fs0() -> u64;
}

/// Zen 5 events (perf list --details).
const EVENTS: &[(&str, u64)] = &[
    ("stlf", 0x35),            // ls_stlf: store-to-load forward hits
    ("stli", 0x0224),          // ls_bad_status2.stli_other: load blocked by a non-forwardable older store
    ("resync", 0x019f),        // bp_redirects.resync: pipeline restarts (memory-order clears among them)
    ("flushrec", 0x04af),      // dispatch stalled on a pending execution flush recovery
    ("ldwait", 0xa2d6),        // ex_no_retire.load_not_complete: oldest op waits for load data
    ("notcomp", 0x02d6),       // ex_no_retire.not_complete
    ("disp", 0x07aa),          // de_src_op_disp.all: ops dispatched
    ("retops", 0xc1),          // ex_ret_ops: ops retired
    ("brmiss", 0xc3),          // ex_ret_brn_misp
    ("ocmiss", 0x2_0000_048f), // op_cache_hit_miss.op_cache_miss (event 0x28f, umask 4)
    ("ocall", 0x2_0000_078f),  // op_cache_hit_miss.all_op_cache_accesses (0x28f, umask 7)
    ("noops", 0xa9),           // de_op_queue_empty
];

struct Counter {
    fd: i32,
}

impl Counter {
    fn new(typ: u32, config: u64) -> Counter {
        let mut attr = [0u64; 16];
        attr[0] = (typ as u64) | (128u64 << 32);
        attr[1] = config;
        // read_format: time enabled | time running
        attr[4] = 1 | 2;
        // disabled | exclude_kernel | exclude_hv
        attr[5] = 1 | (1 << 5) | (1 << 6);
        let fd = unsafe { syscall(298, attr.as_ptr(), 0i32, -1i32, -1i32, 0u64) };
        assert!(fd >= 0, "perf_event_open {typ}:{config:#x} failed");
        Counter { fd: fd as i32 }
    }
    fn start(&self) {
        unsafe {
            ioctl(self.fd, 0x2403, 0);
            ioctl(self.fd, 0x2400, 0);
        }
    }
    /// The count, or None if the counter was multiplexed.
    fn stop(&self) -> Option<u64> {
        let mut v = [0u64; 3];
        unsafe {
            ioctl(self.fd, 0x2401, 0);
            read(self.fd, v.as_mut_ptr() as *mut core::ffi::c_void, 24);
        }
        (v[1] == v[2]).then_some(v[0])
    }
}

fn map(len: usize, align: usize) -> *mut u8 {
    // PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS
    let p = unsafe { mmap(std::ptr::null_mut(), len + align, 3, 0x22, -1, 0) };
    assert!(p as isize != -1);
    ((p as usize + align - 1) & !(align - 1)) as *mut u8
}

struct Env {
    word: *mut u64,
    mag: *mut u64,
    arena: *mut u8,
    root: *mut u64,
    nodes: u64,
    /// tdm: the roots of `FOREST` trees of the same shape as `root`'s, one node per line at
    /// shuffled places in a 128 MiB arena. Built on first use.
    forest: std::cell::OnceCell<*mut u64>,
}

const FOREST: usize = 4096;

impl Env {
    fn new(cpu_id: usize) -> Env {
        let slabs = &raw mut ck_slabs as *mut u8;
        let mag = map(8 * SLOTS, 8 * SLOTS) as *mut u64;
        let arena = map(1 << 16, 1 << 16);
        let root = map(4096, 4096) as *mut u64;
        unsafe {
            for i in 1..SLOTS - 1 {
                mag.add(i).write(arena.add(64 * (i % 64)) as u64);
            }
            // The C routines' tables: class of size C_SIZE, mask of the class, and the heap mirror
            // (shift 16, base index arena >> 16) whose region 0 is the arena, of the class.
            (&raw mut ck_csizes as *mut u8).add(2048 + (C_SIZE + 7) / 8).write(CLASS as u8);
            (slabs.add(33556480 + 8 * CLASS) as *mut u64).write(MASK);
            (slabs.add(33555456) as *mut u32).write(16);
            (slabs.add(33555464) as *mut u64).write(arena as u64 >> 16);
            slabs.add(33560576).write(CLASS as u8);
        }
        // A complete tree of `Node { left, right, value }` (24 bytes) in the arena.
        let mut next = 0usize;
        fn build(arena: *mut u8, next: &mut usize, depth: u32) -> u64 {
            let n = unsafe { arena.add(4096 + 24 * *next) } as *mut u64;
            *next += 1;
            let (l, r) =
                if depth == 0 { (0, 0) } else { (build(arena, next, depth - 1), build(arena, next, depth - 1)) };
            unsafe {
                n.write(l);
                n.add(1).write(r);
                n.add(2).write(*next as u64);
            }
            n as u64
        }
        let t = build(arena, &mut next, DEPTH);
        unsafe { root.write(t) };
        let word = unsafe { slabs.add(cpu_id * 4096 + 8 * CLASS) } as *mut u64;
        Env { word, mag, arena, root, nodes: next as u64, forest: std::cell::OnceCell::new() }
    }

    fn build_forest(&self) -> *mut u64 {
        let per = self.nodes as usize;
        let lines = FOREST * per;
        let arena = map(64 * lines, 4096);
        let roots = map(8 * FOREST, 4096) as *mut u64;
        // A random permutation of the lines, so that consecutive nodes are far apart.
        let mut order: Vec<usize> = (0..lines).collect();
        let mut x = 0x2545_f491_4f6c_dd1du64;
        for i in (1..lines).rev() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            order.swap(i, (x % (i as u64 + 1)) as usize);
        }
        let mut next = 0usize;
        fn build(arena: *mut u8, order: &[usize], next: &mut usize, depth: u32) -> u64 {
            let n = unsafe { arena.add(64 * order[*next]) } as *mut u64;
            *next += 1;
            let (l, r) = if depth == 0 {
                (0, 0)
            } else {
                (build(arena, order, next, depth - 1), build(arena, order, next, depth - 1))
            };
            unsafe {
                n.write(l);
                n.add(1).write(r);
                n.add(2).write(*next as u64);
            }
            n as u64
        }
        for t in 0..FOREST {
            let r = build(arena, &order, &mut next, DEPTH);
            unsafe { roots.add(t).write(r) };
        }
        roots
    }

    /// (reset value of the word, object argument, inner count, operations per inner iteration)
    fn setup(&self, form: &str) -> (u64, u64, u64, u64) {
        let slot = |s: u64| self.mag as u64 + 8 * s;
        let obj = self.arena as u64 + 64;
        match form {
            "if" | "ifg" | "cf" | "sa" | "sb" | "sc" | "sd" | "se" => (slot(1), obj, K, 1),
            "ia" | "cm" | "ip" | "cp" => (slot(K + 1), obj, K, 1),
            "td" | "tdx" => (slot(1), self.root as u64, 1, self.nodes),
            "tdm" => (slot(1), *self.forest.get_or_init(|| self.build_forest()) as u64, FOREST as u64, self.nodes),
            _ => panic!("unknown form {form}"),
        }
    }
}

struct Meas {
    cyc: f64,
    ins: f64,
    ev: Vec<f64>,
}

fn parse_places(s: &str) -> Vec<u32> {
    let mut v = Vec::new();
    for part in s.split(',') {
        if let Some((a, b)) = part.split_once('-') {
            let (a, b): (u32, u32) = (a.parse().unwrap(), b.parse().unwrap());
            v.extend(a..=b);
        } else {
            v.push(part.parse().unwrap());
        }
    }
    v
}

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|s| s.replace('_', "").parse().ok()).unwrap_or(default)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cpu = env_num("RF_CPU", 2) as usize;
    let mask = [1u64 << cpu, 0, 0, 0];
    unsafe { sched_setaffinity(0, 32, mask.as_ptr()) };
    let rseq = (unsafe { ck_fs0() } as isize + unsafe { __rseq_offset }) as *const u32;
    let cpu_id = unsafe { rseq.add(1).read_volatile() } as usize;
    assert!(cpu_id < 8192, "no rseq registration");
    let env = Env::new(cpu_id);

    let forms: Vec<&str> = match args.get(1).map(|s| s.as_str()) {
        None | Some("all") => vec!["if", "ifg", "ia", "ip", "td", "cf", "cm", "cp"],
        Some(s) => s.split(',').collect(),
    };
    let places: Option<Vec<u32>> = args.get(2).map(|s| parse_places(s));
    let events: Vec<(String, u64)> = std::env::var("RF_EVENTS")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| match s.split_once('=') {
            Some((n, c)) => (n.to_string(), u64::from_str_radix(c.trim_start_matches("0x"), 16).unwrap()),
            None => (s.to_string(), EVENTS.iter().find(|e| e.0 == s).unwrap_or_else(|| panic!("no event {s}")).1),
        })
        .collect();
    // Six core counters, one taken by the NMI watchdog, two by cycles and instructions.
    assert!(events.len() <= 3, "at most three events");
    let cycles = Counter::new(0, 0);
    let instrs = Counter::new(0, 1);
    let evc: Vec<Counter> = events.iter().map(|e| Counter::new(4, e.1)).collect();

    let items: Vec<&(&str, Kernel, &str, u32)> =
        KERNELS.iter().filter(|k| forms.contains(&k.2) && places.as_ref().is_none_or(|p| p.contains(&k.3))).collect();
    let ops = env_num("RF_OPS", 2_000_000);
    let reps = env_num("RF_REPS", 3);
    let rounds = env_num("RF_ROUNDS", 3);

    let measure = |k: &(&str, Kernel, &str, u32)| -> Meas {
        let (reset, p, inner, per) = env.setup(k.2);
        let outer = (ops / (inner * per)).max(1);
        let mut best = Meas { cyc: f64::MAX, ins: 0.0, ev: vec![] };
        for _ in 0..reps {
            unsafe { env.word.write(reset) };
            let a0 = unsafe { ck_aborts };
            cycles.start();
            instrs.start();
            evc.iter().for_each(|c| c.start());
            unsafe { (k.1)(outer, env.word, reset, p, inner) };
            let ev: Vec<Option<u64>> = evc.iter().map(|c| c.stop()).collect();
            let i = instrs.stop();
            let c = cycles.stop();
            let aborts = unsafe { ck_aborts } - a0;
            assert_eq!(unsafe { env.word.read() }, reset, "{}: the word moved", k.0);
            let (Some(c), Some(i)) = (c, i) else {
                eprintln!("{}: counters multiplexed", k.0);
                continue;
            };
            if ev.iter().any(Option::is_none) {
                eprintln!("{}: an event counter was multiplexed", k.0);
                continue;
            }
            let d = (outer * inner * per) as f64;
            if aborts * 1000 > outer * inner {
                continue;
            }
            if (c as f64 / d) < best.cyc {
                best = Meas {
                    cyc: c as f64 / d,
                    ins: i as f64 / d,
                    ev: ev.iter().map(|e| e.unwrap() as f64 / d).collect(),
                };
            }
        }
        best
    };

    let mut best: Vec<Option<Meas>> = (0..items.len()).map(|_| None).collect();
    let mut worst = vec![0.0f64; items.len()];
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..rounds {
        let mut order: Vec<usize> = (0..items.len()).collect();
        for i in (1..order.len()).rev() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            order.swap(i, (seed % (i as u64 + 1)) as usize);
        }
        for &i in &order {
            let m = measure(items[i]);
            worst[i] = worst[i].max(m.cyc);
            if best[i].as_ref().is_none_or(|b| m.cyc < b.cyc) {
                best[i] = Some(m);
            }
        }
    }
    let out = std::io::stdout();
    let mut out = out.lock();
    write!(out, "{:<5} {:>5} {:>8} {:>6} {:>7}", "form", "place", "cyc/op", "+", "ins/op").unwrap();
    for e in &events {
        write!(out, " {:>8}", e.0).unwrap();
    }
    writeln!(out).unwrap();
    for (i, k) in items.iter().enumerate() {
        let b = best[i].as_ref().unwrap();
        write!(out, "{:<5} {:>5} {:>8.3} {:>6.3} {:>7.2}", k.2, k.3, b.cyc, worst[i] - b.cyc, b.ins).unwrap();
        for e in &b.ev {
            write!(out, " {:>8.3}", e).unwrap();
        }
        writeln!(out).unwrap();
    }
}
