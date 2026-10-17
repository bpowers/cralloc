//! Tries to break the two guarantees a cache indexed by `mm_cid` rests on:
//!
//! 1. two threads of a process never run user code with the same `mm_cid` at once, and
//! 2. a thread that is preempted, migrated, signalled or given a new `mm_cid` inside a
//!    restartable sequence resumes at the abort handler, never in the middle.
//!
//! Every thread runs one critical section in a loop. It reads its cid, takes the 64-byte slot
//! for that cid, writes its thread id into the slot (a store nobody may rely on), loads the
//! slot's counter, wastes a random number of instructions so that interruptions land inside,
//! checks that the thread id is still its own, and commits `counter + 1` with one store.
//!
//! Two independent detectors:
//!
//! - the thread-id check fails if another thread entered the same slot while this one was
//!   inside and this one was not aborted;
//! - at the end, the sum of all counters must equal the number of commits the threads believe
//!   they made. A lost update (two threads in one slot, or a commit that follows an
//!   unreported interruption) leaves the sum short.
//!
//! The cid is also range-checked against 8192 inside the section.
//!
//! Around that loop the probe can run any mix of disturbances:
//!
//!   flip        thread count swings between `low` and `high` every `phase_ms`, which moves
//!               the process between per-task and per-CPU cid ownership (the kernel switches
//!               to per-CPU above 1.25x the allowed CPUs and back below about half of them)
//!   affinity    a thread re-pins random workers to random CPUs of the starting mask
//!   signals     a thread signals random workers; the handler runs the same critical section
//!   membarrier  a thread issues MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ in a loop
//!   fork        a thread forks; the child checks its cid is 0 and runs the section alone
//!   pagefault   the section reads a page that another thread keeps discarding
//!
//! and one control that must FAIL, to show the detectors can see a real fault:
//!
//!   noarm       the section never tells the kernel it is in a sequence, so a preempted
//!               thread resumes where it stopped. With more threads than CPUs both detectors
//!               fire; with fewer, each thread owns its cid and nothing goes wrong.
//!
//! Usage: cid_stress <seconds> [flip] [affinity] [signals] [membarrier] [fork] [pagefault]
//!                   [noarm] [low=N] [high=N] [phase_ms=N]
//! Run under `taskset` to choose the CPUs; the probe never leaves its starting mask.
//! Exit status is non-zero if either detector fired or a cid was out of range.
use std::arch::asm;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

extern "C" {
    static __rseq_offset: isize;
    static __rseq_size: u32;
    fn syscall(n: i64, ...) -> i64;
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(code: i32) -> !;
    fn sched_getaffinity(pid: i32, len: usize, mask: *mut u64) -> i32;
    fn sched_setaffinity(pid: i32, len: usize, mask: *const u64) -> i32;
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn madvise(addr: *mut u8, len: usize, advice: i32) -> i32;
    fn getpid() -> i32;
}

// glibc's `struct sigaction` on x86-64.
#[repr(C)]
struct SigAction {
    handler: usize,
    mask: [u64; 16],
    flags: i32,
    restorer: usize,
}

const SYS_GETTID: i64 = 186;
const SYS_TGKILL: i64 = 234;
const SYS_MEMBARRIER: i64 = 324;
const MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ: i64 = 1 << 7;
const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ: i64 = 1 << 8;
const SIGUSR1: i32 = 10;
const SA_SIGINFO: i32 = 4;
const SA_RESTART: i32 = 0x1000_0000;
const MADV_DONTNEED: i32 = 4;

const MAX_CID: usize = 8192;
const MAX_WORKERS: usize = 1024;
const MASK_WORDS: usize = 16;

#[repr(C, align(64))]
struct Slot {
    count: UnsafeCell<u64>,
    owner: UnsafeCell<u64>,
}
// Slots are only ever touched from inside the critical section; the point of the probe is
// to find out whether that is as exclusive as the kernel says.
struct Slots([Slot; MAX_CID]);
unsafe impl Sync for Slots {}
static SLOTS: Slots = Slots([const { Slot { count: UnsafeCell::new(0), owner: UnsafeCell::new(0) } }; MAX_CID]);

static TIDS: [AtomicI32; MAX_WORKERS] = [const { AtomicI32::new(0) }; MAX_WORKERS];
static COMMITS: AtomicU64 = AtomicU64::new(0);
static ABORTS: AtomicU64 = AtomicU64::new(0);
static OWNER_VIOLATIONS: AtomicU64 = AtomicU64::new(0);
static BAD_CIDS: AtomicU64 = AtomicU64::new(0);
static WORST_CID: AtomicU64 = AtomicU64::new(0);
static MAX_CID_SEEN: AtomicU64 = AtomicU64::new(0);
// Commits that found another thread's id in the slot, split by which half of a flip cycle
// they happened in. Near zero in per-task mode, large in per-CPU mode: the evidence that the
// ownership mode really changed.
static HANDOFFS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static COMMITS_BY_PHASE: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static PHASE_HIGH: AtomicBool = AtomicBool::new(false);
static HANDLER_RUNS: AtomicU64 = AtomicU64::new(0);
static TOUCH_PAGE: AtomicU64 = AtomicU64::new(0);
static DUMMY_PAGE: u64 = 0;
static ARM: AtomicU64 = AtomicU64::new(u64::MAX);

fn rseq_area() -> *mut u8 {
    let tp: usize;
    unsafe { asm!("mov {}, fs:0", out(reg) tp, options(nostack, readonly)) };
    (tp as isize + unsafe { __rseq_offset }) as *mut u8
}

fn gettid() -> u64 {
    unsafe { syscall(SYS_GETTID) as u64 }
}

struct Outcome {
    res: u64,
    cid: u64,
    prev_owner: u64,
}

/// One attempt at the critical section. `res`: 0 committed, 2 aborted by the kernel,
/// 3 another thread's id appeared in the slot while inside, 4 cid out of range.
#[inline(never)]
unsafe fn cs_inc(rseq: *mut u8, me: u64, delay: u64, page: *const u64) -> Outcome {
    let (res, cid, prev): (u64, u64, u64);
    asm!(
        "lea {cnt}, [rip + 3f]",
        // All ones normally. The `noarm` control passes zero, which stores a null descriptor:
        // the section then runs with no abort protection at all.
        "and {cnt}, {arm}",
        "mov qword ptr [{rseq} + 8], {cnt}",
        "20:",
        "mov {cid:e}, dword ptr [{rseq} + 24]",
        "cmp {cid:e}, 8192",
        "jae 8f",
        "mov {slot}, {cid}",
        "shl {slot}, 6",
        "add {slot}, {slots}",
        "mov {prev}, qword ptr [{slot} + 8]",
        "mov qword ptr [{slot} + 8], {me}",
        "mov {cnt}, qword ptr [{slot}]",
        "21:",
        "test {delay}, {delay}",
        "jz 22f",
        "mov {res}, qword ptr [{page}]",
        "dec {delay}",
        "jmp 21b",
        "22:",
        "cmp qword ptr [{slot} + 8], {me}",
        "jne 9f",
        "lea {cnt}, [{cnt} + 1]",
        "mov qword ptr [{slot}], {cnt}",
        "4:",
        "xor {res:e}, {res:e}",
        "jmp 6f",
        "8:",
        "mov {res:e}, 4",
        "jmp 6f",
        "9:",
        "mov {res:e}, 3",
        "jmp 6f",
        // The kernel jumps to abort_ip only if the four bytes before it hold the signature
        // glibc registered. The three bytes ahead of those make the pair decode as one ud1.
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
        slots = in(reg) SLOTS.0.as_ptr(),
        me = in(reg) me,
        arm = in(reg) ARM.load(Relaxed),
        page = in(reg) page,
        delay = inout(reg) delay => _,
        cid = out(reg) cid,
        slot = out(reg) _,
        cnt = out(reg) _,
        prev = out(reg) prev,
        res = out(reg) res,
        options(nostack),
    );
    Outcome { res, cid, prev_owner: prev }
}

fn note_failure(o: &Outcome, me: u64) {
    if o.res == 3 {
        if OWNER_VIOLATIONS.fetch_add(1, Relaxed) < 5 {
            eprintln!("VIOLATION: tid {me} found another owner in slot {} before its commit", o.cid);
        }
    } else {
        WORST_CID.fetch_max(o.cid, Relaxed);
        if BAD_CIDS.fetch_add(1, Relaxed) < 5 {
            eprintln!("VIOLATION: tid {me} read mm_cid {:#x}", o.cid);
        }
    }
}

extern "C" fn on_signal(_sig: i32, _info: *mut u8, _ctx: *mut u8) {
    HANDLER_RUNS.fetch_add(1, Relaxed);
    let (rseq, me) = (rseq_area(), gettid());
    // Like an allocation made by a signal handler: the same section on the same cid, nested
    // inside whatever the thread was doing, possibly an aborted attempt at this very slot.
    for _ in 0..64 {
        let o = unsafe { cs_inc(rseq, me, 3, &DUMMY_PAGE) };
        match o.res {
            0 => {
                COMMITS.fetch_add(1, Relaxed);
                return;
            }
            2 => {
                ABORTS.fetch_add(1, Relaxed);
            }
            _ => {
                note_failure(&o, me);
                return;
            }
        }
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn worker(idx: usize, stop: Arc<AtomicBool>) {
    let (rseq, me) = (rseq_area(), gettid());
    TIDS[idx].store(me as i32, Relaxed);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (me << 20) ^ idx as u64);
    let (mut commits, mut aborts, mut max_cid) = (0u64, 0u64, 0u64);
    while !stop.load(Relaxed) {
        let phase = PHASE_HIGH.load(Relaxed) as usize;
        let page = TOUCH_PAGE.load(Relaxed) as *const u64;
        let (mut batch_commits, mut handoffs) = (0u64, 0u64);
        for _ in 0..256 {
            let o = unsafe { cs_inc(rseq, me, rng.next() & 63, page) };
            match o.res {
                0 => {
                    batch_commits += 1;
                    handoffs += (o.prev_owner != me) as u64;
                    max_cid = max_cid.max(o.cid);
                }
                2 => aborts += 1,
                _ => note_failure(&o, me),
            }
        }
        commits += batch_commits;
        COMMITS_BY_PHASE[phase].fetch_add(batch_commits, Relaxed);
        HANDOFFS[phase].fetch_add(handoffs, Relaxed);
    }
    TIDS[idx].store(0, Relaxed);
    COMMITS.fetch_add(commits, Relaxed);
    ABORTS.fetch_add(aborts, Relaxed);
    MAX_CID_SEEN.fetch_max(max_cid, Relaxed);
}

fn random_worker(rng: &mut Rng) -> Option<i32> {
    let tid = TIDS[(rng.next() % MAX_WORKERS as u64) as usize].load(Relaxed);
    (tid != 0).then_some(tid)
}

fn affinity_storm(stop: Arc<AtomicBool>, cpus: Vec<usize>, full: [u64; MASK_WORDS]) {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let pick = |rng: &mut Rng, mask: &mut [u64; MASK_WORDS]| {
        let cpu = cpus[(rng.next() % cpus.len() as u64) as usize];
        mask[cpu / 64] |= 1 << (cpu % 64);
    };
    while !stop.load(Relaxed) {
        let Some(tid) = random_worker(&mut rng) else { continue };
        let mut mask = [0u64; MASK_WORDS];
        match rng.next() % 4 {
            0 => mask = full,
            1 => {
                pick(&mut rng, &mut mask);
                pick(&mut rng, &mut mask);
            }
            _ => pick(&mut rng, &mut mask),
        }
        // ESRCH for a worker that has just exited is expected.
        unsafe { sched_setaffinity(tid, MASK_WORDS * 8, mask.as_ptr()) };
    }
}

fn signal_storm(stop: Arc<AtomicBool>) {
    let (mut rng, pid) = (Rng(0xfeed_face_cafe_beef), unsafe { getpid() });
    while !stop.load(Relaxed) {
        if let Some(tid) = random_worker(&mut rng) {
            unsafe { syscall(SYS_TGKILL, pid as i64, tid as i64, SIGUSR1 as i64) };
        }
    }
}

fn membarrier_storm(stop: Arc<AtomicBool>) -> u64 {
    let mut calls = 0;
    while !stop.load(Relaxed) {
        let r = unsafe { syscall(SYS_MEMBARRIER, MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ, 0i64, 0i64) };
        assert_eq!(r, 0, "membarrier failed: {}", std::io::Error::last_os_error());
        calls += 1;
    }
    calls
}

/// Forks, and has the child check what it inherited. Returns (forks, children that failed).
fn fork_storm(stop: Arc<AtomicBool>) -> (u64, u64) {
    let (mut forks, mut failed) = (0u64, 0u64);
    while !stop.load(Relaxed) {
        let pid = unsafe { fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            // Only raw system calls and the critical section from here: the child is a copy
            // of a multithreaded process.
            let (rseq, me) = (rseq_area(), gettid());
            unsafe {
                if (rseq.add(4) as *const i32).read_volatile() < 0 {
                    _exit(10);
                }
                // A new address space with one thread: the kernel must have replaced the
                // parent's cid with 0 before the child's first instruction.
                if (rseq.add(24) as *const u32).read_volatile() != 0 {
                    _exit(11);
                }
                let before = SLOTS.0[0].count.get().read_volatile();
                let mut done = 0;
                for _ in 0..400 {
                    let o = cs_inc(rseq, me, 16, &DUMMY_PAGE);
                    match o.res {
                        0 if o.cid == 0 => done += 1,
                        2 => {}
                        _ => _exit(13),
                    }
                }
                if SLOTS.0[0].count.get().read_volatile() != before + done {
                    _exit(12);
                }
                _exit(0);
            }
        }
        let mut status = 0;
        unsafe { waitpid(pid, &mut status, 0) };
        forks += 1;
        if status != 0 {
            failed += 1;
            if failed <= 5 {
                eprintln!("VIOLATION: forked child reported status {:#x} (exit code {})", status, (status >> 8) & 0xff);
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    (forks, failed)
}

fn pagefault_storm(stop: Arc<AtomicBool>, page: *mut u8) {
    while !stop.load(Relaxed) {
        unsafe { madvise(page, 4096, MADV_DONTNEED) };
        std::thread::sleep(Duration::from_micros(50));
    }
}

struct SendPtr(*mut u8);
unsafe impl Send for SendPtr {}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seconds: u64 = args.first().and_then(|s| s.parse().ok()).expect("usage: cid_stress <seconds> [modes]");
    let has = |name: &str| args.iter().any(|a| a == name);
    let num = |name: &str, default: usize| {
        args.iter().find_map(|a| a.strip_prefix(name)?.strip_prefix('=')?.parse().ok()).unwrap_or(default)
    };
    assert!(unsafe { __rseq_size } >= 28, "no rseq registration with mm_cid");

    let mut full = [0u64; MASK_WORDS];
    assert!(unsafe { sched_getaffinity(0, MASK_WORDS * 8, full.as_mut_ptr()) } == 0);
    let cpus: Vec<usize> = (0..MASK_WORDS * 64).filter(|c| full[c / 64] >> (c % 64) & 1 == 1).collect();
    let low = num("low", (cpus.len() / 3).max(2));
    let high = num("high", cpus.len() * 3).min(MAX_WORKERS);
    let phase_ms = num("phase_ms", 20) as u64;
    let flip = has("flip");
    println!("cpus={} low={low} high={high} phase_ms={phase_ms} modes={:?}", cpus.len(), &args[1..]);

    if has("signals") {
        let act = SigAction { handler: on_signal as *const () as usize, mask: [0; 16], flags: SA_SIGINFO | SA_RESTART, restorer: 0 };
        assert_eq!(unsafe { sigaction(SIGUSR1, &act, std::ptr::null_mut()) }, 0);
    }
    TOUCH_PAGE.store(&DUMMY_PAGE as *const u64 as u64, Relaxed);
    if has("noarm") {
        ARM.store(0, Relaxed);
    }

    let stop = Arc::new(AtomicBool::new(false));
    let mut helpers = Vec::new();
    if has("affinity") {
        let (s, c) = (stop.clone(), cpus.clone());
        helpers.push(std::thread::spawn(move || affinity_storm(s, c, full)));
    }
    if has("signals") {
        let s = stop.clone();
        helpers.push(std::thread::spawn(move || signal_storm(s)));
    }
    if has("pagefault") {
        // PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS
        let page = unsafe { mmap(std::ptr::null_mut(), 4096, 1, 0x22, -1, 0) };
        assert!(page as isize != -1);
        TOUCH_PAGE.store(page as u64, Relaxed);
        let (s, p) = (stop.clone(), SendPtr(page));
        helpers.push(std::thread::spawn(move || {
            let p = p;
            pagefault_storm(s, p.0)
        }));
    }
    let barrier = has("membarrier").then(|| {
        let r = unsafe { syscall(SYS_MEMBARRIER, MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ, 0i64, 0i64) };
        assert_eq!(r, 0, "membarrier registration failed");
        let s = stop.clone();
        std::thread::spawn(move || membarrier_storm(s))
    });
    let forker = has("fork").then(|| {
        let s = stop.clone();
        std::thread::spawn(move || fork_storm(s))
    });

    // The worker pool is a stack: growing pushes new threads, shrinking stops and joins the
    // newest. With `flip` its size alternates between `low` and `high`.
    let mut pool: Vec<(std::thread::JoinHandle<()>, Arc<AtomicBool>)> = Vec::new();
    let resize = |pool: &mut Vec<(std::thread::JoinHandle<()>, Arc<AtomicBool>)>, target: usize| {
        while pool.len() < target {
            let (idx, flag) = (pool.len(), Arc::new(AtomicBool::new(false)));
            let f = flag.clone();
            pool.push((std::thread::spawn(move || worker(idx, f)), flag));
        }
        for (_, flag) in &pool[target..] {
            flag.store(true, Relaxed);
        }
        for (handle, _) in pool.drain(target..) {
            handle.join().unwrap();
        }
    };

    let (start, mut flips) = (Instant::now(), 0u64);
    let deadline = Duration::from_secs(seconds);
    resize(&mut pool, if flip { low } else { high });
    while start.elapsed() < deadline {
        if flip {
            let to_high = flips % 2 == 0;
            // The phase flag changes while the pool is at its smaller size, so that the
            // commits made during a resize are attributed to the larger phase's mode at
            // worst, never the other way round.
            if !to_high {
                resize(&mut pool, low);
            }
            PHASE_HIGH.store(to_high, Relaxed);
            if to_high {
                resize(&mut pool, high);
            }
            flips += 1;
        }
        std::thread::sleep(Duration::from_millis(phase_ms));
    }
    resize(&mut pool, 0);
    stop.store(true, Relaxed);
    for h in helpers {
        h.join().unwrap();
    }
    let barrier_calls = barrier.map(|h| h.join().unwrap());
    let forks = forker.map(|h| h.join().unwrap());

    let sum: u64 = SLOTS.0.iter().map(|s| unsafe { s.count.get().read() }).sum();
    let used = SLOTS.0.iter().filter(|s| unsafe { s.count.get().read() } != 0).count();
    let commits = COMMITS.load(Relaxed);
    let (owner, bad) = (OWNER_VIOLATIONS.load(Relaxed), BAD_CIDS.load(Relaxed));
    println!(
        "commits={commits} counter_sum={sum} lost={} aborts={} owner_violations={owner} bad_cids={bad} worst_bad_cid={:#x}",
        commits as i64 - sum as i64,
        ABORTS.load(Relaxed),
        WORST_CID.load(Relaxed),
    );
    println!("slots used={used} max cid={} flips={flips} handler runs={}", MAX_CID_SEEN.load(Relaxed), HANDLER_RUNS.load(Relaxed));
    for (name, i) in [("low ", 0), ("high", 1)] {
        let (c, h) = (COMMITS_BY_PHASE[i].load(Relaxed), HANDOFFS[i].load(Relaxed));
        println!("phase {name}: commits={c} handoffs={h} ({:.4}%)", 100.0 * h as f64 / c.max(1) as f64);
    }
    if let Some(calls) = barrier_calls {
        println!("membarrier calls={calls} ({:.1} us each)", seconds as f64 * 1e6 / calls.max(1) as f64);
    }
    let fork_failures = forks.map_or(0, |(n, failed)| {
        println!("forks={n} failed children={failed}");
        failed
    });
    if commits != sum || owner != 0 || bad != 0 || fork_failures != 0 {
        println!("FAIL");
        std::process::exit(1);
    }
    println!("ok");
}
