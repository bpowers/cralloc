//! What the kernel charges an allocator, on a kernel of any page size.
//!
//! ```text
//! kcost purge CPU[,OTHER...] [reps]  MADV_DONTNEED of 1 to 512 pages; OTHER CPUs run threads of the
//!                                    process, and the lost laps are what the call costs them
//! kcost calls CPU                    the rest of the family, each timed in a loop
//! kcost barrier CPU TARGET,...       membarrier with the rseq command, by CPU and for all, with a
//!                                    thread of the process spinning on each target
//! ```
//!
//! Times come from the generic timer. At 24 MHz one read is 42 ns, so a call under a microsecond is
//! timed over a loop; `purge` times each call, since it faults its pages back in between.
use roofline_machine::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

fn purge(cpus: &[usize], reps: usize) {
    let page = page();
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
                let buf = Map::resident(64 * page, Pages::Small);
                while !stop.load(Relaxed) {
                    let mut o = 0;
                    while o < buf.len {
                        // SAFETY: inside this thread's own mapping.
                        unsafe { buf.p.add(o).write_volatile(buf.p.add(o).read_volatile().wrapping_add(1)) };
                        o += page;
                    }
                    lap.fetch_add(1, Relaxed);
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(100));
    let map = Map::new(1024 * page, Pages::Small);
    let total = || laps.iter().map(|l| l.load(Relaxed)).sum::<u64>();
    let idle_rate = {
        let before = total();
        std::thread::sleep(Duration::from_millis(200));
        (total() - before) as f64 / 0.2
    };
    println!(
        "# on CPU {}, page {page}, other threads on {:?}, their laps/s with no calls {idle_rate:.0}",
        cpus[0],
        &cpus[1..]
    );
    println!("# pages  KiB  call_us_p50  call_us_p90  ns_per_page  ns_per_KiB  lost_laps_per_call  fault_us_per_page");
    for pages in [1usize, 2, 4, 8, 16, 32, 64, 128, 512] {
        let len = pages * page;
        let mut times = Vec::with_capacity(reps);
        let mut faulting = 0u64;
        let before = total();
        let start = ticks();
        for _ in 0..reps {
            let t0 = ticks();
            let mut o = 0;
            while o < len {
                // SAFETY: inside the mapping.
                unsafe { map.p.add(o).write_volatile(1) };
                o += page;
            }
            let t1 = ticks();
            assert_eq!(adv(map.p, len, MADV_DONTNEED), 0);
            let t2 = ticks();
            faulting += t1 - t0;
            times.push(ns_of(t2 - t1) / 1e3);
        }
        let elapsed = ns_of(ticks() - start) / 1e9;
        let lost = if cpus.len() > 1 { (idle_rate * elapsed - (total() - before) as f64) / reps as f64 } else { 0.0 };
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (p50, p90) = (times[reps / 2], times[reps * 9 / 10]);
        println!(
            "{pages:6} {:5} {p50:9.2} {p90:9.2} {:9.1} {:9.1} {lost:9.2} {:9.3}",
            len >> 10,
            p50 * 1e3 / pages as f64,
            p50 * 1e3 / (len >> 10) as f64,
            ns_of(faulting) / 1e3 / (reps * pages) as f64,
        );
    }
    stop.store(true, Relaxed);
    workers.into_iter().for_each(|w| w.join().unwrap());
}

/// Nanoseconds an iteration of `f`, the median of five timed loops, after one to warm up.
fn loop_ns(n: usize, mut f: impl FnMut(usize)) -> f64 {
    let mut rounds = vec![];
    for round in 0..6 {
        let t0 = ticks();
        for i in 0..n {
            f(i);
        }
        if round > 0 {
            rounds.push(ns_of(ticks() - t0) / n as f64);
        }
    }
    median(&mut rounds)
}

fn calls(cpu: usize) {
    let page = page();
    pin(cpu);
    println!("# CPU {cpu}, page {page}, huge {}: ns a call", huge());
    let show = |name: &str, ns: f64| println!("{name:58} {ns:10.1}");
    show(
        "empty loop",
        loop_ns(1 << 20, |i| {
            std::hint::black_box(i);
        }),
    );
    let mut sum = 0u64;
    show(
        "cntvct_el0 read, plain",
        loop_ns(1 << 20, |_| {
            let t: u64;
            // SAFETY: a read of the timer at EL0.
            unsafe { std::arch::asm!("mrs {t}, cntvct_el0", t = out(reg) t, options(nostack, nomem, preserves_flags)) };
            sum = sum.wrapping_add(t);
        }),
    );
    show(
        "cntvct_el0 read, after isb",
        loop_ns(1 << 20, |_| {
            sum = sum.wrapping_add(ticks());
        }),
    );
    std::hint::black_box(sum);
    show(
        "clock_gettime (vDSO), monotonic",
        loop_ns(1 << 18, |_| {
            std::hint::black_box(std::time::Instant::now());
        }),
    );
    const SYS_GETPPID: i64 = 173;
    // SAFETY: a system call with no arguments.
    show(
        "getppid, the cost of entering the kernel",
        loop_ns(1 << 18, |_| unsafe {
            syscall(SYS_GETPPID);
        }),
    );
    let word = AtomicU64::new(0);
    const FUTEX_WAKE_PRIVATE: i64 = 1 | 128;
    // SAFETY: the futex word is a live u32-aligned word; nobody waits on it.
    show(
        "futex wake, no waiter",
        loop_ns(1 << 18, |_| unsafe {
            syscall(SYS_FUTEX, word.as_ptr(), FUTEX_WAKE_PRIVATE, 1i64, 0i64, 0i64, 0i64);
        }),
    );

    let many = 2048 * page;
    let map = Map::new(many, Pages::Small);
    let p = map.p;
    let mut residency = vec![0u8; 256];
    map.touch();
    // SAFETY: inside the mapping; the vector holds a byte for each page asked about.
    show(
        "mincore, 1 resident page",
        loop_ns(1 << 16, |_| unsafe {
            mincore(p, page, residency.as_mut_ptr());
        }),
    );
    // SAFETY: as above.
    show(
        "mincore, 16 resident pages",
        loop_ns(1 << 16, |_| unsafe {
            mincore(p, 16 * page, residency.as_mut_ptr());
        }),
    );
    // SAFETY: as above.
    show(
        "mincore, 256 resident pages",
        loop_ns(1 << 14, |_| unsafe {
            mincore(p, 256 * page, residency.as_mut_ptr());
        }),
    );
    show(
        "MADV_HUGEPAGE then MADV_NOHUGEPAGE of part of a mapping, the pair",
        loop_ns(1 << 12, |_| {
            adv(p.wrapping_add(64 * page), 128 * page, MADV_HUGEPAGE);
            adv(p.wrapping_add(64 * page), 128 * page, MADV_NOHUGEPAGE);
        }),
    );
    assert_eq!(adv(p, many, MADV_DONTNEED), 0);
    show(
        "MADV_DONTNEED, 1 page, nothing resident",
        loop_ns(1 << 16, |_| {
            adv(p, page, MADV_DONTNEED);
        }),
    );
    show(
        "MADV_DONTNEED, 64 pages, nothing resident",
        loop_ns(1 << 16, |_| {
            adv(p, 64 * page, MADV_DONTNEED);
        }),
    );
    // Each of the next calls acts on resident pages, so the page is written again first; a write
    // that faults is timed with it, and the row after it says how much that is.
    show(
        "write 1 fresh page, MADV_DONTNEED it",
        loop_ns(1 << 14, |_| {
            // SAFETY: inside the mapping.
            unsafe { p.write_volatile(1) };
            adv(p, page, MADV_DONTNEED);
        }),
    );
    show(
        "write 1 fresh page, MADV_FREE it",
        loop_ns(1 << 14, |_| {
            // SAFETY: inside the mapping.
            unsafe { p.write_volatile(1) };
            adv(p, page, MADV_FREE);
        }),
    );
    show(
        "write 1 fresh page, MADV_COLD it",
        loop_ns(1 << 14, |_| {
            // SAFETY: inside the mapping.
            unsafe { p.write_volatile(1) };
            adv(p, page, MADV_COLD);
        }),
    );
    adv(p, many, MADV_DONTNEED);
    for pages in [1usize, 16, 64] {
        map.touch();
        // Touching pages that MADV_FREE left lazily free costs a write, not a fault.
        show(
            &format!("MADV_FREE and rewrite, {pages} resident pages"),
            loop_ns(1 << 13, |_| {
                adv(p, pages * page, MADV_FREE);
                let mut o = 0;
                while o < pages * page {
                    // SAFETY: inside the mapping.
                    unsafe { p.add(o).write_volatile(2) };
                    o += page;
                }
            }),
        );
        show(
            &format!("MADV_COLD alone, {pages} resident pages"),
            loop_ns(1 << 13, |_| {
                adv(p, pages * page, MADV_COLD);
            }),
        );
    }
    adv(p, many, MADV_DONTNEED);
    // SAFETY: inside the mapping.
    show(
        "mprotect none and back, 128 untouched pages, the pair",
        loop_ns(1 << 13, |_| unsafe {
            mprotect(p.wrapping_add(64 * page), 128 * page, 0);
            mprotect(p.wrapping_add(64 * page), 128 * page, PROT_RW);
        }),
    );
    map.touch();
    // SAFETY: inside the mapping.
    show(
        "mprotect none and back, 128 resident pages, the pair",
        loop_ns(1 << 13, |_| unsafe {
            mprotect(p.wrapping_add(64 * page), 128 * page, 0);
            mprotect(p.wrapping_add(64 * page), 128 * page, PROT_RW);
        }),
    );
    let span = 128 * page;
    // SAFETY: a fresh mapping, unmapped at once.
    show(
        "mmap and munmap, 128 untouched pages, the pair",
        loop_ns(1 << 14, |_| unsafe {
            let q = mmap(std::ptr::null_mut(), span, PROT_RW, MAP_PRIVATE_ANON, -1, 0);
            munmap(q, span);
        }),
    );
    // SAFETY: a fresh mapping, written inside itself, unmapped at once.
    show(
        "mmap, write 4 pages, munmap",
        loop_ns(1 << 12, |_| unsafe {
            let q = mmap(std::ptr::null_mut(), span, PROT_RW, MAP_PRIVATE_ANON, -1, 0);
            for k in 0..4 {
                q.add(k * page).write_volatile(1);
            }
            munmap(q, span);
        }),
    );
}

fn barrier(cpu: usize, targets: &[usize]) {
    const CMD_PRIVATE_EXPEDITED_RSEQ: i64 = 1 << 7;
    const CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ: i64 = 1 << 8;
    const FLAG_CPU: i64 = 1;
    pin(cpu);
    // SAFETY: the register command takes no memory.
    let r = unsafe { syscall(SYS_MEMBARRIER, CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ, 0i64, 0i64) };
    assert_eq!(r, 0, "membarrier register: errno {}", errno());
    println!("# from CPU {cpu}: ns a call");
    // SAFETY: the command names a CPU and takes no memory.
    let by_cpu = |t: usize| {
        loop_ns(1 << 14, |_| unsafe {
            syscall(SYS_MEMBARRIER, CMD_PRIVATE_EXPEDITED_RSEQ, FLAG_CPU, t as i64);
        })
    };
    // SAFETY: as above.
    let for_all = || {
        loop_ns(1 << 14, |_| unsafe {
            syscall(SYS_MEMBARRIER, CMD_PRIVATE_EXPEDITED_RSEQ, 0i64, 0i64);
        })
    };
    for &t in targets {
        println!("by CPU, target {t}, no thread of ours there       {:10.1}", by_cpu(t));
    }
    println!("for all, no other thread of ours running         {:10.1}", for_all());
    let stop = Arc::new(AtomicBool::new(false));
    let spinners: Vec<_> = targets
        .iter()
        .map(|&t| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                pin(t);
                while !stop.load(Relaxed) {}
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(50));
    for &t in targets {
        println!("by CPU, target {t}, a thread of ours spinning there {:10.1}", by_cpu(t));
    }
    println!("for all, threads of ours spinning on {targets:?} {:10.1}", for_all());
    stop.store(true, Relaxed);
    spinners.into_iter().for_each(|h| h.join().unwrap());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let num = |i: usize, d: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let list = |i: usize| parse_cpus(args.get(i).map_or("8", String::as_str));
    match args.get(1).map(String::as_str) {
        Some("purge") => purge(&list(2), num(3, 400)),
        Some("calls") => calls(num(2, 8)),
        Some("barrier") => barrier(num(2, 8), &list(3)),
        _ => eprintln!("usage: kcost purge|calls|barrier ..."),
    }
}
