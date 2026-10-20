//! What one `madvise(MADV_DONTNEED)` costs against how many pages it returns, and what it does
//! to the process's other threads through TLB shootdowns.
//!
//! The main thread, pinned to the first CPU of the list, faults in a run of `pages` pages and
//! returns it with one call, many times, and reports the median and 90th percentile time of
//! the call. Each other CPU of the list runs a thread of the same process that loops over a
//! private buffer and counts its laps, so that the process's address space is live on those
//! CPUs and every call must shoot down their TLBs; the probe reports the laps they lost per
//! call against a run with no calls.
//!
//!   rustc --edition 2024 -O madvise_cost.rs -o DIR/madvise_cost
//!   madvise_cost CPU,CPU,... [REPS]
//!
//! Run it pinned off other work, under scripts/measure and scripts/memlimit.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const PAGE: usize = 4096;
const MADV_DONTNEED: i32 = 4;
const PROT_RW: i32 = 3;
const MAP_PRIVATE_ANON: i32 = 0x22;

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
    fn sched_setaffinity(pid: i32, size: usize, mask: *const u64) -> i32;
}

fn pin(cpu: usize) {
    let mut mask = [0u64; 16];
    mask[cpu / 64] |= 1 << (cpu % 64);
    // SAFETY: the mask is a valid cpu_set_t of 1024 bits for the calling thread.
    unsafe { sched_setaffinity(0, std::mem::size_of_val(&mask), mask.as_ptr()) };
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cpus: Vec<usize> = args[1].split(',').map(|c| c.parse().unwrap()).collect();
    let reps: usize = args.get(2).map_or(400, |r| r.parse().unwrap());
    pin(cpus[0]);
    let stop = Arc::new(AtomicBool::new(false));
    let laps: Vec<Arc<AtomicU64>> = cpus[1..].iter().map(|_| Arc::new(AtomicU64::new(0))).collect();
    let workers: Vec<_> = cpus[1..]
        .iter()
        .zip(&laps)
        .map(|(&cpu, lap)| {
            let (stop, lap) = (stop.clone(), lap.clone());
            std::thread::spawn(move || {
                pin(cpu);
                // A buffer of 64 pages, walked a line a page: each lap touches 64 TLB entries.
                let mut buf = vec![0u8; 64 * PAGE];
                while !stop.load(Ordering::Relaxed) {
                    for p in (0..buf.len()).step_by(PAGE) {
                        buf[p] = buf[p].wrapping_add(1);
                    }
                    lap.fetch_add(1, Ordering::Relaxed);
                }
                std::hint::black_box(&buf);
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(100));
    let max = 512;
    // SAFETY: a fresh private anonymous mapping, used only by this thread.
    let base = unsafe { mmap(std::ptr::null_mut(), max * PAGE, PROT_RW, MAP_PRIVATE_ANON, -1, 0) };
    assert!(!base.is_null() && base as isize != -1);
    let lap_rate = |d: Duration| {
        let before: u64 = laps.iter().map(|l| l.load(Ordering::Relaxed)).sum();
        std::thread::sleep(d);
        let after: u64 = laps.iter().map(|l| l.load(Ordering::Relaxed)).sum();
        (after - before) as f64 / d.as_secs_f64()
    };
    let idle_rate = lap_rate(Duration::from_millis(200));
    println!("other threads: {}  laps/s with no calls: {idle_rate:.0}", cpus.len() - 1);
    println!("pages  call_us_p50  call_us_p90  us_per_page  lost_laps_per_call");
    for pages in [1usize, 2, 4, 8, 16, 32, 64, 128, 512] {
        let len = pages * PAGE;
        let mut times = Vec::with_capacity(reps);
        let before: u64 = laps.iter().map(|l| l.load(Ordering::Relaxed)).sum();
        let start = Instant::now();
        let mut in_calls = Duration::ZERO;
        for _ in 0..reps {
            for p in (0..len).step_by(PAGE) {
                // SAFETY: inside the mapping.
                unsafe { base.add(p).write_volatile(1) };
            }
            let t = Instant::now();
            // SAFETY: the range is inside the mapping.
            let r = unsafe { madvise(base, len, MADV_DONTNEED) };
            let d = t.elapsed();
            assert_eq!(r, 0);
            in_calls += d;
            times.push(d.as_nanos() as f64 / 1e3);
        }
        let elapsed = start.elapsed();
        let after: u64 = laps.iter().map(|l| l.load(Ordering::Relaxed)).sum();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        // Laps the others would have run in the time, against those they ran.
        let lost = if cpus.len() > 1 {
            (idle_rate * elapsed.as_secs_f64() - (after - before) as f64) / reps as f64
        } else {
            0.0
        };
        let _ = in_calls;
        println!(
            "{pages:5}  {:11.2}  {:11.2}  {:11.3}  {lost:18.2}",
            times[reps / 2],
            times[reps * 9 / 10],
            times[reps / 2] / pages as f64
        );
    }
    stop.store(true, Ordering::Relaxed);
    for w in workers {
        w.join().unwrap();
    }
}
