//! Checks the one promise the idle-cache drain needs from `membarrier`: once
//! `MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ` has returned, no restartable sequence that was
//! in flight when it was called can still commit, whether its thread was running on another
//! CPU or sitting preempted in the middle of the sequence.
//!
//! Workers, several per CPU, increment a per-cid word by 2 inside a critical section. The
//! section refuses to commit over the value 1 (the marker). A drainer walks the cids and
//! runs the drain protocol from the design on each word:
//!
//!   1. swap the marker in, keeping the old value;
//!   2. call membarrier;
//!   3. read the word. Still the marker: the drainer owns the old value, and from now on
//!      nothing may write the word. It watches the word for a while and then stores the old
//!      value back. Anything else: a sequence that had loaded the word before step 1
//!      committed; that is allowed, and the drainer takes nothing.
//!
//! A write that lands while the drainer believes it owns the word is a late commit: the
//! guarantee is broken. A commit later still, after the old value went back, overwrites
//! other threads' increments, so the final sum of the words is also compared with the number
//! of commits.
//!
//! `nobarrier` skips step 2 and must FAIL; it shows the probe can see the fault.
//!
//! Usage: rseq_drain <seconds> [threads=N] [nobarrier]
use std::arch::asm;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed, Ordering::SeqCst};
use std::sync::Arc;
use std::time::{Duration, Instant};

extern "C" {
    static __rseq_offset: isize;
    static __rseq_size: u32;
    fn syscall(n: i64, ...) -> i64;
}

#[cfg(target_arch = "x86_64")]
const SYS_MEMBARRIER: i64 = 324;
#[cfg(target_arch = "aarch64")]
const SYS_MEMBARRIER: i64 = 283;
const MEMBARRIER_CMD_QUERY: i64 = 0;
const MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ: i64 = 1 << 7;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ: i64 = 1 << 8;
const MAX_CID: usize = 8192;
const MARKER: u64 = 1;

// A line on either architecture.
#[repr(C, align(128))]
struct Slot(AtomicU64);
static SLOTS: [Slot; MAX_CID] = [const { Slot(AtomicU64::new(0)) }; MAX_CID];
static COMMITS: AtomicU64 = AtomicU64::new(0);
static ABORTS: AtomicU64 = AtomicU64::new(0);
static MARKER_SEEN: AtomicU64 = AtomicU64::new(0);
static MAX_CID_SEEN: AtomicU64 = AtomicU64::new(0);

fn rseq_area() -> *mut u8 {
    let tp: usize;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        asm!("mov {}, fs:0", out(reg) tp, options(nostack, readonly))
    };
    #[cfg(target_arch = "aarch64")]
    unsafe {
        asm!("mrs {}, tpidr_el0", out(reg) tp, options(nomem, nostack, preserves_flags))
    };
    (tp as isize + unsafe { __rseq_offset }) as *mut u8
}

fn membarrier(cmd: i64) -> i64 {
    unsafe { syscall(SYS_MEMBARRIER, cmd, 0i64, 0i64) }
}

/// Add 2 to the word of the current cid unless it holds the marker.
/// Returns (0 committed | 1 marker | 2 aborted | 4 cid out of range, cid).
#[cfg(target_arch = "aarch64")]
#[inline(never)]
unsafe fn cs_add(rseq: *mut u8, delay: u64) -> (u64, u64) {
    let (res, cid): (u64, u64);
    asm!(
        "adrp {cnt}, 3f",
        "add {cnt}, {cnt}, :lo12:3f",
        "str {cnt}, [{rseq}, #8]",
        "20:",
        "ldr {cid:w}, [{rseq}, #24]",
        "cmp {cid:w}, #2, lsl #12",
        "b.hs 8f",
        "add {slot}, {slots}, {cid}, lsl #7",
        "ldr {cnt}, [{slot}]",
        "cmp {cnt}, #1",
        "b.eq 9f",
        "21:",
        "cbz {delay}, 22f",
        "sub {delay}, {delay}, #1",
        "b 21b",
        "22:",
        "add {cnt}, {cnt}, #2",
        "str {cnt}, [{slot}]",
        "4:",
        "mov {res}, #0",
        "b 6f",
        "8:",
        "mov {res}, #4",
        "b 6f",
        "9:",
        "mov {res}, #1",
        "b 6f",
        ".inst 0xd428bc00",
        "7:",
        "mov {res}, #2",
        "6:",
        ".pushsection __rseq_cs, \"aw\"",
        ".balign 32",
        "3:",
        ".long 0, 0",
        ".quad 20b, 4b - 20b, 7b",
        ".popsection",
        rseq = in(reg) rseq,
        slots = in(reg) SLOTS.as_ptr(),
        delay = inout(reg) delay => _,
        cid = out(reg) cid,
        slot = out(reg) _,
        cnt = out(reg) _,
        res = out(reg) res,
        options(nostack),
    );
    (res, cid)
}

/// The same section on x86-64.
#[cfg(target_arch = "x86_64")]
#[inline(never)]
unsafe fn cs_add(rseq: *mut u8, delay: u64) -> (u64, u64) {
    let (res, cid): (u64, u64);
    asm!(
        "lea {cnt}, [rip + 3f]",
        "mov qword ptr [{rseq} + 8], {cnt}",
        "20:",
        "mov {cid:e}, dword ptr [{rseq} + 24]",
        "cmp {cid:e}, 8192",
        "jae 8f",
        "mov {slot}, {cid}",
        "shl {slot}, 7",
        "add {slot}, {slots}",
        "mov {cnt}, qword ptr [{slot}]",
        "cmp {cnt}, 1",
        "je 9f",
        "21:",
        "test {delay}, {delay}",
        "jz 22f",
        "dec {delay}",
        "jmp 21b",
        "22:",
        "lea {cnt}, [{cnt} + 2]",
        "mov qword ptr [{slot}], {cnt}",
        "4:",
        "xor {res:e}, {res:e}",
        "jmp 6f",
        "8:",
        "mov {res:e}, 4",
        "jmp 6f",
        "9:",
        "mov {res:e}, 1",
        "jmp 6f",
        ".byte 0x0f, 0xb9, 0x3d",
        ".long 0x53053053",
        "7:",
        "mov {res:e}, 2",
        "6:",
        ".pushsection __rseq_cs, \"aw\"",
        ".balign 32",
        "3:",
        ".long 0, 0",
        ".quad 20b, 4b - 20b, 7b",
        ".popsection",
        rseq = in(reg) rseq,
        slots = in(reg) SLOTS.as_ptr(),
        delay = inout(reg) delay => _,
        cid = out(reg) cid,
        slot = out(reg) _,
        cnt = out(reg) _,
        res = out(reg) res,
        options(nostack),
    );
    (res, cid)
}

fn worker(stop: Arc<AtomicBool>, seed: u64) {
    let rseq = rseq_area();
    let mut x = seed | 1;
    let (mut commits, mut aborts, mut marker, mut max_cid) = (0u64, 0u64, 0u64, 0u64);
    while !stop.load(Relaxed) {
        for _ in 0..256 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            match unsafe { cs_add(rseq, x & 127) } {
                (0, cid) => {
                    commits += 1;
                    max_cid = max_cid.max(cid);
                }
                (1, _) => marker += 1,
                (2, _) => aborts += 1,
                (_, cid) => panic!("mm_cid {cid:#x} out of range"),
            }
        }
    }
    COMMITS.fetch_add(commits, Relaxed);
    ABORTS.fetch_add(aborts, Relaxed);
    MARKER_SEEN.fetch_add(marker, Relaxed);
    MAX_CID_SEEN.fetch_max(max_cid, Relaxed);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seconds: u64 = args.first().and_then(|s| s.parse().ok()).expect("usage: rseq_drain <seconds> [threads=N] [nobarrier]");
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads: usize = args.iter().find_map(|a| a.strip_prefix("threads=")?.parse().ok()).unwrap_or(cpus * 3);
    let use_barrier = !args.iter().any(|a| a == "nobarrier");
    assert!(unsafe { __rseq_size } >= 28, "no rseq registration with mm_cid");

    let supported = membarrier(MEMBARRIER_CMD_QUERY);
    let before = membarrier(MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ);
    let before_errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    let reg = membarrier(MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ);
    println!(
        "membarrier query={supported:#x} (rseq command {}), before registering: ret={before} errno={before_errno}, register: ret={reg}",
        if supported & MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ != 0 { "offered" } else { "missing" }
    );
    assert_eq!(reg, 0);

    let stop = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = (0..threads)
        .map(|i| {
            let s = stop.clone();
            std::thread::spawn(move || worker(s, 0x9e37_79b9_7f4a_7c15u64.wrapping_mul(i as u64 + 1)))
        })
        .collect();

    let (mut owned, mut raced, mut late, mut anomalies, mut calls, mut call_ns) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u128);
    let start = Instant::now();
    // Cids never exceed the CPU count; a margin covers the transient ids seen while the
    // kernel changes ownership mode.
    let scan = (cpus * 2).min(MAX_CID);
    while start.elapsed() < Duration::from_secs(seconds) {
        for slot in &SLOTS[..scan] {
            let old = slot.0.swap(MARKER, SeqCst);
            if use_barrier {
                let t = Instant::now();
                let r = membarrier(MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ);
                call_ns += t.elapsed().as_nanos();
                calls += 1;
                assert_eq!(r, 0, "membarrier failed: {}", std::io::Error::last_os_error());
            }
            let now = slot.0.load(SeqCst);
            if now != MARKER {
                // A sequence that loaded `old` before the swap committed after it.
                raced += 1;
                anomalies += (now < old + 2 || now & 1 != 0) as u64;
                continue;
            }
            owned += 1;
            let mut overwritten = false;
            for _ in 0..2000 {
                if slot.0.load(Relaxed) != MARKER {
                    overwritten = true;
                    break;
                }
                std::hint::spin_loop();
            }
            if overwritten {
                late += 1;
                if late <= 5 {
                    eprintln!("VIOLATION: a sequence committed after the drainer took ownership of the word");
                }
            } else {
                slot.0.store(old, SeqCst);
            }
        }
    }
    stop.store(true, Relaxed);
    for w in workers {
        w.join().unwrap();
    }
    let sum: u64 = SLOTS.iter().map(|s| s.0.load(Relaxed)).sum();
    let commits = COMMITS.load(Relaxed);
    println!(
        "threads={threads} cpus={cpus} barrier={use_barrier} drains: owned={owned} lost-race={raced} late-commits={late} anomalies={anomalies}"
    );
    println!(
        "commits={commits} word_sum/2={} lost={} aborts={} marker-seen={} max cid={}",
        sum / 2,
        commits as i64 - (sum / 2) as i64,
        ABORTS.load(Relaxed),
        MARKER_SEEN.load(Relaxed),
        MAX_CID_SEEN.load(Relaxed)
    );
    if calls > 0 {
        println!("membarrier: {calls} calls, {:.1} us each", call_ns as f64 / calls as f64 / 1000.0);
    }
    if late != 0 || anomalies != 0 || commits * 2 != sum {
        println!("FAIL");
        std::process::exit(1);
    }
    println!("ok");
}
