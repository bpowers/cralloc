//! Allocation workloads through Rust's global allocator API, one binary per allocator.
//!
//!   roofline-rust-alloc [label] [workload-prefix]
//!
//! The allocator is chosen at build time by cargo feature (see Cargo.toml). Single-thread
//! workloads report user-space cycles and instructions per operation on CPU RF_CPU (default 2).
//! The cross-thread workloads put the allocating thread on RF_CPU and the freeing thread on
//! RF_CPU2 (default 3), and report the time each side spends working, per object. RF_BATCH
//! (default 32) is the burst size and RF_PAIRS (default 3) the number of pairs in `x_cross_pairs`.
#[cfg(any(feature = "floor_tls", feature = "floor_heap"))]
mod floors;
#[cfg(feature = "slab")]
mod slab;

#[allow(unused_imports)]
use std::alloc::{alloc, dealloc, GlobalAlloc, Layout};
use std::hint::black_box;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::Arc;
use std::time::Instant;

/// mimalloc, called the way the `mimalloc` crate calls it (`mi_malloc_aligned` and `mi_free`), or
/// with the size passed back on free (feature `mi_sized`).
#[cfg(feature = "mi")]
mod mi {
    use super::*;
    extern "C" {
        fn mi_malloc(size: usize) -> *mut u8;
        fn mi_malloc_aligned(size: usize, align: usize) -> *mut u8;
        fn mi_free(p: *mut u8);
        fn mi_free_size_aligned(p: *mut u8, size: usize, align: usize);
    }
    pub struct Mi;
    unsafe impl GlobalAlloc for Mi {
        #[inline]
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            if cfg!(feature = "mi_sized") {
                mi_malloc(l.size())
            } else {
                mi_malloc_aligned(l.size(), l.align())
            }
        }
        #[inline]
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            if cfg!(feature = "mi_sized") {
                mi_free_size_aligned(p, l.size(), l.align())
            } else {
                mi_free(p)
            }
        }
    }
}

/// jemalloc, called the way `tikv-jemallocator` calls it: `malloc` or `mallocx` to allocate,
/// `sdallocx` (sized) to free.
#[cfg(feature = "je")]
mod je {
    use super::*;
    #[link(name = "jemalloc")]
    extern "C" {
        #[allow(suspicious_runtime_symbol_definitions)]
        fn malloc(size: usize) -> *mut u8;
        fn mallocx(size: usize, flags: i32) -> *mut u8;
        fn sdallocx(p: *mut u8, size: usize, flags: i32);
    }
    #[inline(always)]
    fn flags(l: Layout) -> i32 {
        if l.align() <= 16 && l.align() <= l.size() {
            0
        } else {
            l.align().trailing_zeros() as i32
        }
    }
    pub struct Je;
    unsafe impl GlobalAlloc for Je {
        #[inline]
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let f = flags(l);
            if f == 0 {
                malloc(l.size())
            } else {
                mallocx(l.size(), f)
            }
        }
        #[inline]
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            sdallocx(p, l.size(), flags(l))
        }
    }
}

#[cfg(feature = "mi")]
#[global_allocator]
static A: mi::Mi = mi::Mi;
#[cfg(feature = "je")]
#[global_allocator]
static A: je::Je = je::Je;
#[cfg(any(feature = "floor_tls", feature = "floor_heap"))]
#[global_allocator]
static A: floors::Floor = floors::Floor;
#[cfg(feature = "asm")]
#[global_allocator]
static A: asmalloc::Asmalloc = asmalloc::Asmalloc;
#[cfg(feature = "slab")]
#[global_allocator]
static A: slab::Slab = slab::Slab;

extern "C" {
    fn syscall(num: i64, ...) -> i64;
    fn ioctl(fd: i32, req: u64, ...) -> i32;
    fn sched_setaffinity(pid: i32, sz: usize, mask: *const u64) -> i32;
}

fn pin(cpu: usize) {
    let mut mask = [0u64; 4];
    mask[cpu / 64] = 1 << (cpu % 64);
    unsafe { sched_setaffinity(0, 32, mask.as_ptr()) };
}

fn env(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

/// A user-space hardware counter for the calling thread.
struct Counter(std::fs::File);

impl Counter {
    fn new(config: u64) -> Counter {
        use std::os::fd::FromRawFd;
        let mut attr = [0u64; 16];
        attr[0] = 128u64 << 32;
        attr[1] = config;
        // disabled | exclude_kernel | exclude_hv
        attr[5] = 1 | (1 << 5) | (1 << 6);
        let fd = unsafe { syscall(298, attr.as_ptr(), 0i32, -1i32, -1i32, 0u64) };
        assert!(fd >= 0, "perf_event_open failed");
        Counter(unsafe { std::fs::File::from_raw_fd(fd as i32) })
    }
    fn start(&self) {
        use std::os::fd::AsRawFd;
        unsafe {
            ioctl(self.0.as_raw_fd(), 0x2403, 0);
            ioctl(self.0.as_raw_fd(), 0x2400, 0);
        }
    }
    fn stop(&mut self) -> u64 {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        unsafe { ioctl(self.0.as_raw_fd(), 0x2401, 0) };
        let mut b = [0u8; 8];
        self.0.read_exact(&mut b).unwrap();
        u64::from_le_bytes(b)
    }
}

const L32: Layout = unsafe { Layout::from_size_align_unchecked(32, 8) };

/// Allocate, write, free, with a layout known at compile time.
#[inline(never)]
fn pair_const(n: u64) -> u64 {
    let mut acc = 0u64;
    for i in 0..n {
        unsafe {
            let p = alloc(L32) as *mut u64;
            p.write(i);
            acc = acc.wrapping_add(p as u64 ^ p.read());
            dealloc(p as *mut u8, L32);
        }
    }
    acc
}

/// The same through `Box`, with the pointer escaping so the compiler cannot see through it.
#[inline(never)]
fn pair_box(n: u64) -> u64 {
    let mut acc = 0u64;
    for i in 0..n {
        let b = black_box(Box::new([i, 1, 2, 3]));
        acc = acc.wrapping_add(b[0]);
    }
    acc
}

/// The same with a size only known at run time.
#[inline(never)]
fn pair_dyn(n: u64, size: usize) -> u64 {
    let mut acc = 0u64;
    for i in 0..n {
        unsafe {
            let l = Layout::from_size_align_unchecked(black_box(size), 8);
            let p = alloc(l) as *mut u64;
            p.write(i);
            acc = acc.wrapping_add(p as u64 ^ p.read());
            dealloc(p as *mut u8, l);
        }
    }
    acc
}

/// 32 allocations, then 32 frees in allocation order.
#[inline(never)]
fn batch_const(n: u64) -> u64 {
    let mut acc = 0u64;
    let mut v = [std::ptr::null_mut::<u64>(); 32];
    for i in 0..n / 32 {
        unsafe {
            for s in v.iter_mut() {
                let p = alloc(L32) as *mut u64;
                p.write(i);
                *s = p;
            }
            for s in v.iter() {
                acc = acc.wrapping_add(*s as u64);
                dealloc(*s as *mut u8, L32);
            }
        }
    }
    acc
}

/// 10,000 live objects; each step frees a pseudo-random one and allocates a replacement of a
/// pseudo-random size in 16..=512.
#[inline(never)]
fn churn(n: u64) -> u64 {
    let mut live: Vec<(*mut u8, Layout)> = Vec::with_capacity(10_000);
    let mut x = 0x9E3779B97F4A7C15u64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let lay = |r: u64| unsafe { Layout::from_size_align_unchecked(16 + ((r >> 20) as usize % 63) * 8, 8) };
    unsafe {
        for _ in 0..10_000 {
            let l = lay(next());
            live.push((alloc(l), l));
        }
        let mut acc = 0u64;
        for _ in 0..n {
            let r = next();
            let i = (r % 10_000) as usize;
            let (p, l) = live[i];
            dealloc(p, l);
            let l2 = lay(r);
            let p2 = alloc(l2);
            (p2 as *mut u64).write(r);
            acc = acc.wrapping_add(p2 as u64);
            live[i] = (p2, l2);
        }
        for (p, l) in live {
            dealloc(p, l)
        }
        acc
    }
}

fn single_thread(label: &str, filter: &str) {
    pin(env("RF_CPU", 2));
    let mut cyc = Counter::new(0);
    let mut ins = Counter::new(1);
    let n = 20_000_000u64;
    let mut run = |what: &str, f: &dyn Fn() -> u64| {
        if !what.starts_with(filter) {
            return;
        }
        let mut best = (f64::MAX, 0.0, 0.0);
        for rep in 0..4 {
            cyc.start();
            ins.start();
            let t = Instant::now();
            black_box(f());
            let ns = t.elapsed().as_nanos() as f64;
            let i = ins.stop() as f64;
            let c = cyc.stop() as f64;
            // The first repetition warms up.
            if rep > 0 && c < best.0 {
                best = (c, i, ns)
            }
        }
        let d = n as f64;
        println!("{label:<18} {what:<12} {:>7.2} cyc/op {:>7.2} ins/op {:>7.3} ns/op", best.0 / d, best.1 / d, best.2 / d);
    };
    run("pair_const", &|| pair_const(n));
    run("pair_box", &|| pair_box(n));
    run("pair_dyn", &|| pair_dyn(n, 32));
    run("batch_const", &|| batch_const(n));
    run("churn", &|| churn(n));
}

// ---- cross-thread: one thread allocates, another frees ----

const RING: usize = 8192;

/// Objects allocated (and published) per burst: RF_BATCH, default 32. Bursts larger than a
/// thread cache's capacity force every allocator to its shared structures.
static BATCH_SIZE: AtomicUsize = AtomicUsize::new(32);

#[inline(always)]
fn batch() -> usize {
    BATCH_SIZE.load(Relaxed)
}

#[repr(align(128))]
struct Padded<T>(T);

/// Single-producer single-consumer ring of pointers, published a batch at a time so its own
/// cost is small and the same for every allocator.
struct Ring {
    slots: Box<[AtomicPtr<u8>]>,
    head: Padded<AtomicUsize>,
    tail: Padded<AtomicUsize>,
    /// Time stamp counter ticks the consumer spent freeing (not waiting).
    free_ticks: Padded<AtomicU64>,
}

fn tsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Runs `objects` allocations of `layout` on this thread (pinned to `cpus.0`) and their frees on
/// another (pinned to `cpus.1`). If `touch` the consumer reads the object before freeing it, as
/// code that received a message would. Returns (alloc ticks, free ticks) spent working.
fn produce_consume(cpus: (usize, usize), objects: usize, layout: Layout, touch: bool) -> (u64, u64) {
    let ring = Arc::new(Ring {
        slots: (0..RING).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect(),
        head: Padded(AtomicUsize::new(0)),
        tail: Padded(AtomicUsize::new(0)),
        free_ticks: Padded(AtomicU64::new(0)),
    });
    let r = ring.clone();
    let consumer = std::thread::spawn(move || {
        pin(cpus.1);
        let mut tail = 0usize;
        let mut ticks = 0u64;
        let mut acc = 0u64;
        while tail < objects {
            let head = r.head.0.load(Acquire);
            if head == tail {
                std::hint::spin_loop();
                continue;
            }
            let t0 = tsc();
            while tail < head {
                let p = r.slots[tail % RING].load(Relaxed);
                unsafe {
                    if touch {
                        acc = acc.wrapping_add((p as *const u64).read());
                    }
                    dealloc(p, layout);
                }
                tail += 1;
            }
            ticks += tsc() - t0;
            r.tail.0.store(tail, Release);
        }
        black_box(acc);
        r.free_ticks.0.store(ticks, Release);
    });
    pin(cpus.0);
    let mut head = 0usize;
    let mut ticks = 0u64;
    while head < objects {
        while head + batch() - ring.tail.0.load(Acquire) > RING {
            std::hint::spin_loop();
        }
        let t0 = tsc();
        for i in 0..batch() {
            unsafe {
                let p = alloc(layout);
                (p as *mut u64).write(head as u64);
                ring.slots[(head + i) % RING].store(p, Relaxed);
            }
        }
        ticks += tsc() - t0;
        head += batch();
        ring.head.0.store(head, Release);
    }
    consumer.join().unwrap();
    (ticks, ring.free_ticks.0.load(Acquire))
}

/// A ring of threads, each allocating objects for the next thread and freeing the ones it gets
/// from the previous: every thread allocates and frees at the same rate, but never frees what
/// it allocated. This is the shape of work-stealing and channel-heavy servers. Returns the
/// ticks spent allocating and freeing, summed over the threads.
fn exchange(cpus: &[usize], objects: usize, layout: Layout, touch: bool) -> (u64, u64) {
    let k = cpus.len();
    let rings: Vec<Arc<Ring>> = (0..k)
        .map(|_| {
            Arc::new(Ring {
                slots: (0..RING).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect(),
                head: Padded(AtomicUsize::new(0)),
                tail: Padded(AtomicUsize::new(0)),
                free_ticks: Padded(AtomicU64::new(0)),
            })
        })
        .collect();
    let handles: Vec<_> = (0..k)
        .map(|i| {
            let (out, inp, cpu) = (rings[i].clone(), rings[(i + k - 1) % k].clone(), cpus[i]);
            std::thread::spawn(move || {
                pin(cpu);
                let (mut head, mut tail) = (0usize, 0usize);
                let (mut ta, mut tf) = (0u64, 0u64);
                let mut acc = 0u64;
                while head < objects || tail < objects {
                    let mut worked = false;
                    if head < objects && head + batch() - out.tail.0.load(Acquire) <= RING {
                        let t0 = tsc();
                        for j in 0..batch() {
                            unsafe {
                                let p = alloc(layout);
                                (p as *mut u64).write(head as u64);
                                out.slots[(head + j) % RING].store(p, Relaxed);
                            }
                        }
                        ta += tsc() - t0;
                        head += batch();
                        out.head.0.store(head, Release);
                        worked = true;
                    }
                    let avail = inp.head.0.load(Acquire);
                    if tail < avail {
                        let t0 = tsc();
                        while tail < avail {
                            let p = inp.slots[tail % RING].load(Relaxed);
                            unsafe {
                                if touch {
                                    acc = acc.wrapping_add((p as *const u64).read());
                                }
                                dealloc(p, layout);
                            }
                            tail += 1;
                        }
                        tf += tsc() - t0;
                        inp.tail.0.store(tail, Release);
                        worked = true;
                    }
                    if !worked {
                        std::hint::spin_loop();
                    }
                }
                black_box(acc);
                (ta, tf)
            })
        })
        .collect();
    let mut sum = (0u64, 0u64);
    for h in handles {
        let (a, f) = h.join().unwrap();
        sum.0 += a;
        sum.1 += f;
    }
    sum
}

/// The same amount of work with no hand-off: allocate a batch, then free it, on one thread.
fn produce_consume_local(cpu: usize, objects: usize, layout: Layout) -> (u64, u64) {
    pin(cpu);
    let mut slots = vec![std::ptr::null_mut::<u8>(); batch()];
    let (mut a, mut f) = (0u64, 0u64);
    let mut done = 0;
    while done < objects {
        let t0 = tsc();
        for (i, s) in slots.iter_mut().enumerate() {
            unsafe {
                let p = alloc(layout);
                (p as *mut u64).write((done + i) as u64);
                *s = p;
            }
        }
        let t1 = tsc();
        for s in slots.iter() {
            unsafe { dealloc(*s, layout) }
        }
        f += tsc() - t1;
        a += t1 - t0;
        done += batch();
    }
    (a, f)
}

fn cross_thread(label: &str, filter: &str) {
    BATCH_SIZE.store(env("RF_BATCH", 32), Relaxed);
    let (a, b) = (env("RF_CPU", 2), env("RF_CPU2", 3));
    let objects = 20_000_000usize / batch() * batch();
    // Time stamp counter ticks per nanosecond, measured over a short sleep.
    let (t0, c0) = (Instant::now(), tsc());
    std::thread::sleep(std::time::Duration::from_millis(50));
    let ticks_per_ns = (tsc() - c0) as f64 / t0.elapsed().as_nanos() as f64;
    let report = |what: &str, f: &dyn Fn() -> (u64, u64)| {
        if !what.starts_with(filter) {
            return;
        }
        let mut best = (f64::MAX, 0.0, 0.0);
        for rep in 0..4 {
            let t = Instant::now();
            let (ta, tf) = f();
            let wall = t.elapsed().as_nanos() as f64 / objects as f64;
            let (ta, tf) = (ta as f64 / ticks_per_ns / objects as f64, tf as f64 / ticks_per_ns / objects as f64);
            if rep > 0 && ta + tf < best.0 + best.1 {
                best = (ta, tf, wall)
            }
        }
        println!(
            "{label:<18} {what:<18} alloc {:>6.2} ns  free {:>6.2} ns  sum {:>6.2} ns  (wall {:>6.2} ns/object)",
            best.0,
            best.1,
            best.0 + best.1,
            best.2
        );
    };
    report("x_local", &|| produce_consume_local(a, objects, L32));
    report("x_cross", &|| produce_consume((a, b), objects, L32, false));
    report("x_cross_touch", &|| produce_consume((a, b), objects, L32, true));
    // Symmetric exchange between two threads, then around a ring of four on a, b, a+2, b+2.
    report("x_swap2", &|| exchange(&[a, b], objects / 2, L32, true));
    report("x_swap4", &|| exchange(&[a, b, a + 2, b + 2], objects / 4, L32, true));
    // Several producer/consumer pairs at once, on CPUs (a, b), (a+2, b+2), ...
    let pairs = env("RF_PAIRS", 3);
    report("x_cross_pairs", &|| {
        let per = objects / pairs / batch() * batch();
        let handles: Vec<_> = (0..pairs)
            .map(|i| std::thread::spawn(move || produce_consume((a + 2 * i, b + 2 * i), per, L32, true)))
            .collect();
        let mut sum = (0u64, 0u64);
        for h in handles {
            let (x, y) = h.join().unwrap();
            // Scaled so the report's division by `objects` gives a per-object figure.
            sum.0 += x * (objects / per) as u64 / pairs as u64;
            sum.1 += y * (objects / per) as u64 / pairs as u64;
        }
        sum
    });
}

fn main() {
    let label = std::env::args().nth(1).unwrap_or_default();
    let filter = std::env::args().nth(2).unwrap_or_default();
    single_thread(&label, &filter);
    cross_thread(&label, &filter);
}
