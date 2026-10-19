//! What clock a short job gets, as a function of what the package did just before it.
//!
//! Each trial: stay idle for `pause_ms` (so the package settles), spin on the `warm` CPUs for
//! `warm_ms`, sleep `gap_ms`, then spin on the `job` CPUs for `measure_ms`, reading each
//! spinning thread's cycle counter every `window_ms`. Cycles over wall time of a thread that
//! never sleeps is its clock. One line per trial: the clock of the first window, the mean
//! over the first 80 ms and over the whole spin, and when the clock first reached `hi` GHz.
//! `trace=1` prints every window as well.
//!
//! Usage (pinned elsewhere, under the measurement lock):
//!   clock_ramp job=2 warm=none warm_ms=0 gap_ms=0 pause_ms=1000 measure_ms=300 reps=5
//!   clock_ramp job=0-3 warm=14 warm_ms=500 gap_ms=50 ...
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[repr(C)]
struct CpuSet([u64; 16]);

unsafe extern "C" {
    fn syscall(n: i64, ...) -> i64;
    fn sched_setaffinity(pid: i32, size: usize, set: *const CpuSet) -> i32;
    fn read(fd: i32, buf: *mut std::ffi::c_void, n: usize) -> isize;
    fn close(fd: i32) -> i32;
}

const SYS_PERF_EVENT_OPEN: i64 = 298;

fn pin(cpu: usize) {
    let mut s = CpuSet([0; 16]);
    s.0[cpu / 64] |= 1 << (cpu % 64);
    // SAFETY: a valid cpu set of the size passed.
    unsafe { sched_setaffinity(0, std::mem::size_of::<CpuSet>(), &s) };
}

/// A cycle counter on the calling thread, user and kernel, counting from now.
fn cycles_counter() -> i32 {
    let mut attr = [0u64; 16];
    attr[0] = 128u64 << 32; // type 0 (hardware), size 128
    attr[1] = 0; // cycles
    attr[5] = 1 << 6; // exclude_hv
    // SAFETY: attr is a perf_event_attr of the size it declares.
    let fd = unsafe { syscall(SYS_PERF_EVENT_OPEN, attr.as_ptr(), 0i32, -1i32, -1i32, 8u64) };
    assert!(fd >= 0, "perf_event_open failed");
    fd as i32
}

fn read_counter(fd: i32) -> u64 {
    let mut v = 0u64;
    // SAFETY: reads 8 bytes into v.
    unsafe { read(fd, (&mut v as *mut u64).cast(), 8) };
    v
}

fn parse_cpus(s: &str) -> Vec<usize> {
    if s == "none" {
        return vec![];
    }
    let mut v = vec![];
    for p in s.split(',') {
        match p.split_once('-') {
            Some((a, b)) => v.extend(a.parse::<usize>().unwrap()..=b.parse().unwrap()),
            None => v.push(p.parse().unwrap()),
        }
    }
    v
}

/// Busy jiffies and total jiffies per CPU.
fn cpu_times() -> Vec<(u64, u64)> {
    let t = std::fs::read_to_string("/proc/stat").unwrap();
    let mut v = vec![];
    for l in t.lines().filter(|l| l.starts_with("cpu") && l.as_bytes()[3].is_ascii_digit()) {
        let f: Vec<u64> = l.split_whitespace().skip(1).map(|x| x.parse().unwrap()).collect();
        let total: u64 = f.iter().take(8).sum();
        v.push((total - f[3] - f[4], total));
    }
    v
}

fn busy_others(a: &[(u64, u64)], b: &[(u64, u64)], skip: &[usize]) -> f64 {
    let mut s = 0.0;
    for i in 0..a.len().min(b.len()) {
        if skip.contains(&i) || b[i].1 <= a[i].1 {
            continue;
        }
        s += (b[i].0 - a[i].0) as f64 / (b[i].1 - a[i].1) as f64;
    }
    s
}

fn spin_on(cpus: &[usize], dur: Duration) {
    let hs: Vec<_> = cpus
        .iter()
        .map(|&c| {
            std::thread::spawn(move || {
                pin(c);
                let t = Instant::now();
                while t.elapsed() < dur {
                    std::hint::spin_loop();
                }
            })
        })
        .collect();
    hs.into_iter().for_each(|h| h.join().unwrap());
}

/// Each job CPU's (elapsed ns, cycles) at every window boundary.
fn measure(cpus: &[usize], dur: Duration, window: Duration) -> Vec<Vec<(u64, u64)>> {
    let go = Arc::new(AtomicBool::new(false));
    let hs: Vec<_> = cpus
        .iter()
        .map(|&c| {
            let go = go.clone();
            std::thread::spawn(move || {
                pin(c);
                let fd = cycles_counter();
                while !go.load(Ordering::Acquire) {
                    std::hint::spin_loop();
                }
                let t0 = Instant::now();
                let c0 = read_counter(fd);
                let mut pts = vec![];
                let mut next = window;
                loop {
                    let e = t0.elapsed();
                    if e >= next {
                        pts.push((e.as_nanos() as u64, read_counter(fd) - c0));
                        next += window;
                        if e >= dur {
                            break;
                        }
                    }
                }
                // SAFETY: our own descriptor.
                unsafe { close(fd) };
                pts
            })
        })
        .collect();
    // The threads start spinning on their own CPUs; give them a moment to be placed, which
    // is short against any ramp being looked for.
    std::thread::sleep(Duration::from_micros(200));
    go.store(true, Ordering::Release);
    hs.into_iter().map(|h| h.join().unwrap()).collect()
}

fn main() {
    let mut job = vec![2];
    let mut warm = vec![];
    let (mut warm_ms, mut gap_ms, mut pause_ms, mut measure_ms, mut window_ms, mut reps) = (0, 0, 1000, 300, 2, 5);
    let mut trace = false;
    let mut hi = 5.4;
    for a in std::env::args().skip(1) {
        let (k, v) = a.split_once('=').expect("key=value");
        match k {
            "job" => job = parse_cpus(v),
            "warm" => warm = parse_cpus(v),
            "warm_ms" => warm_ms = v.parse().unwrap(),
            "gap_ms" => gap_ms = v.parse().unwrap(),
            "pause_ms" => pause_ms = v.parse().unwrap(),
            "measure_ms" => measure_ms = v.parse().unwrap(),
            "window_ms" => window_ms = v.parse().unwrap(),
            "reps" => reps = v.parse().unwrap(),
            "trace" => trace = v == "1",
            "hi" => hi = v.parse().unwrap(),
            _ => panic!("unknown {k}"),
        }
    }
    let ms = Duration::from_millis;
    let mut skip: Vec<usize> = job.clone();
    skip.extend(&warm);
    for rep in 0..reps {
        let a = cpu_times();
        std::thread::sleep(ms(pause_ms));
        let b = cpu_times();
        let others = busy_others(&a, &b, &skip);
        if warm_ms > 0 && !warm.is_empty() {
            spin_on(&warm, ms(warm_ms));
        }
        std::thread::sleep(ms(gap_ms));
        let per = measure(&job, ms(measure_ms), ms(window_ms));
        // Mean over job CPUs at each window.
        let n = per.iter().map(Vec::len).min().unwrap();
        let ghz_at = |i: usize| -> f64 {
            per.iter()
                .map(|p| {
                    let (t0, c0) = if i == 0 { (0, 0) } else { p[i - 1] };
                    (p[i].1 - c0) as f64 / (p[i].0 - t0) as f64
                })
                .sum::<f64>()
                / per.len() as f64
        };
        let mean_to = |t_ms: u64| -> f64 {
            per.iter()
                .map(|p| {
                    let k = p.iter().position(|x| x.0 >= t_ms * 1_000_000).unwrap_or(p.len() - 1);
                    p[k].1 as f64 / p[k].0 as f64
                })
                .sum::<f64>()
                / per.len() as f64
        };
        let t_hi = (0..n).find(|&i| ghz_at(i) >= hi).map(|i| per[0][i].0 as f64 / 1e6);
        println!(
            "rep {rep} job {job:?} warm {warm:?} warm_ms {warm_ms} gap_ms {gap_ms} others_busy {others:.2}: \
             first {:.2} GHz, 0-80ms {:.2}, all {:.2}, reached {hi} at {}",
            ghz_at(0),
            mean_to(80),
            mean_to(measure_ms),
            t_hi.map(|t| format!("{t:.0} ms")).unwrap_or_else(|| "never".into())
        );
        if trace {
            let line: Vec<String> = (0..n).map(|i| format!("{:.2}", ghz_at(i))).collect();
            println!("  trace: {}", line.join(" "));
        }
    }
}
