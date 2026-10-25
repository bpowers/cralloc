//! Long runs at a large heap, for docs/research/heap-drift.md: does an allocator's resident
//! memory creep up against the bytes the program holds as the size mix, the lifetimes and the
//! set of busy threads change?
//!
//!   workload drift shape=<steady|shift|lifetime|swing|large> [key=value ...]
//!
//! The run is `samples` epochs of `ops` operations in all. Every epoch belongs to one of
//! `phases` phases, which is what changes over the run; at the end of each epoch every thread
//! stops at a barrier and thread 0 prints a `DRIFT` line with the program's live bytes and
//! the process's resident memory. The work is driven and sampled by operation count, not by
//! time, so a process that is paused or slowed does the same work and is sampled at the same
//! points. After the last epoch the threads stop, the live set stays, and the process is
//! sampled every `quiet_step` seconds for `quiet` seconds.
//!
//! Every thread is pinned (thread `i` to the `i`-th CPU of `BENCH_CPUS`, or of the CPUs the
//! process may use; `pin=0` leaves them free), and each has its own seeded generator, so each
//! allocator serves the same requests from each thread. Which thread frees a cached object
//! depends on the interleaving.
//!
//! The shapes:
//!
//! - `steady`: a cache of `live` bytes of values (log-normal sizes, median `median`, shape
//!   `sigma`, at most `max`), shared by all threads: each operation puts a new value into a
//!   random slot and evicts random entries while the cache is over its budget, so most frees
//!   are of another thread's object. Each operation also allocates `transients` short-lived
//!   objects (log-normal, median 96 B) that live for the thread's next 64 of them.
//! - `shift`: the same, with the value median moving through `medians`, one per phase.
//! - `lifetime`: a FIFO of short-lived values on each thread whose total target alternates
//!   between `short_hi` and `short_lo` by phase, and among them, `long_permille` in a
//!   thousand values of the same sizes that go into a long-lived cache of `long` bytes
//!   instead, which turns over slowly by random replacement.
//! - `swing`: the steady cache, filled by producers and drained by consumers through a queue,
//!   with the number of busy threads taken from `active` by phase (half of them producers);
//!   the others wait at the barrier.
//! - `large`: a cache of `live` bytes of buffers, log-uniform in the phase's range of
//!   `ranges`; `grow` percent of them are built by doubling with `realloc` from 4 KiB (half of
//!   those then shrunk to their exact size), the rest allocated whole, and `append` percent
//!   of operations instead take a random cached buffer and grow it by half with `realloc`;
//!   `big_permille` in a thousand new buffers are `big_factor` times the size drawn.
//!
//! Phases that ask for the same sizes draw the same table of sizes (`tables=phase` draws one
//! for every phase instead).
//!
//! Every value is written in full when it is made. The program's own bookkeeping (the caches'
//! slot tables, the FIFOs, the transient rings) is mapped with `mmap`, outside the allocator,
//! and its resident pages are subtracted from the process's: `heap` in a `DRIFT` line is
//! `RssAnon` less that bookkeeping, and `ratio` is `heap` over `live`. `cgroup` is the
//! process's cgroup's `memory.current`, everything the kernel charges for it.
//!
//! With `account_every=N`, a build that can break its heap down (asmalloc's `memstat`) does
//! so at every `N`-th sample, at the last sample of the load and at the last quiet one, while
//! every thread is stopped, so the breakdown is exact.
use bench_harness::report::Measure;
use bench_harness::{Args, Better, Counters, Cpus, Report, Rng};
use std::alloc::{Layout, alloc, dealloc, realloc};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

const PAGE: usize = 4096;
/// A cache slot packs a block's address (8-aligned, below 2^47) and its size in units.
const ADDR_BITS: u32 = 44;
const SIZE_BITS: u32 = 64 - ADDR_BITS;
/// Bytes a thread's count of a cache's live bytes may drift from the shared count before it
/// is added in: the budget holds to within this times the threads.
const FLUSH: i64 = 1 << 20;
/// How many slots past a random one an eviction looks for an entry.
const PROBE: usize = 64;
/// The transients a thread keeps: each lives for its next this many.
const RING: usize = 64;
/// The size a buffer that `large` builds by doubling starts at.
const GROW_FROM: usize = 4096;

fn layout(size: usize) -> Layout {
    Layout::from_size_align(size, 8).expect("a valid layout")
}

/// Allocates `size` bytes and writes all of them, as a program fills what it asks for.
fn make(size: usize, fill: u8) -> *mut u8 {
    // SAFETY: the size is not zero; the write stays inside the block.
    unsafe {
        let p = alloc(layout(size));
        assert!(!p.is_null(), "allocation of {size} bytes failed");
        p.write_bytes(fill, size);
        p
    }
}

fn free(p: *mut u8, size: usize) {
    // SAFETY: every caller passes a block from `make` or `resize` with its size.
    unsafe { dealloc(p, layout(size)) }
}

/// Resizes a block from `old` to `new` bytes and writes what it gained.
fn resize(p: *mut u8, old: usize, new: usize) -> *mut u8 {
    // SAFETY: `p` holds `old` bytes with alignment 8; `new` is not zero; the write stays
    // inside the new block.
    unsafe {
        let q = realloc(p, layout(old), new);
        assert!(!q.is_null(), "realloc to {new} bytes failed");
        if new > old {
            q.add(old).write_bytes(0x5a, new - old);
        }
        q
    }
}

fn address(p: *mut u8) -> usize {
    p.expose_provenance()
}

fn block(addr: usize) -> *mut u8 {
    std::ptr::with_exposed_provenance_mut(addr)
}

/// Memory of the program's own, from `mmap`, so that it is not the allocator's.
struct Mapped {
    base: *mut u8,
    len: usize,
}

// SAFETY: the mapping is plain memory that lives as long as the value; what is shared
// through it is atomics.
unsafe impl Send for Mapped {}
unsafe impl Sync for Mapped {}

impl Mapped {
    fn new(len: usize) -> Mapped {
        let len = len.max(PAGE).next_multiple_of(PAGE);
        // SAFETY: a fresh private anonymous mapping.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        assert!(p != libc::MAP_FAILED, "mmap of {len} bytes failed");
        Mapped { base: p.cast(), len }
    }

    fn resident(&self) -> u64 {
        resident(address(self.base), self.len)
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: unmaps this value's own mapping.
        unsafe { libc::munmap(self.base.cast(), self.len) };
    }
}

/// The resident bytes of a mapping of the program's own, from `mincore`.
fn resident(base: usize, len: usize) -> u64 {
    let mut pages = vec![0u8; len.div_ceil(PAGE)];
    // SAFETY: mincore only reads the page tables; `pages` has a byte per page of the range.
    let rc = unsafe { libc::mincore(block(base).cast(), len, pages.as_mut_ptr()) };
    if rc != 0 {
        return 0;
    }
    pages.iter().filter(|&&b| b & 1 != 0).count() as u64 * PAGE as u64
}

/// A count that one thread writes and another reads, on a line of its own.
#[repr(align(128))]
#[derive(Default)]
struct Line(AtomicI64);

/// A cache of blocks under a budget of live bytes: a table of slots, each empty or a block,
/// that any thread may put into or evict from.
struct Cache {
    /// Which of a thread's pending counts is this cache's.
    id: usize,
    slots: Mapped,
    cap: usize,
    unit: usize,
    budget: i64,
    live: Line,
}

impl Cache {
    fn new(id: usize, budget: usize, cap: usize, unit: usize) -> Cache {
        let cap = cap.max(1024);
        Cache { id, slots: Mapped::new(cap * 8), cap, unit, budget: budget as i64, live: Line::default() }
    }

    fn slot(&self, i: usize) -> &AtomicU64 {
        debug_assert!(i < self.cap);
        // SAFETY: the mapping holds `cap` zero-initialized u64s, and AtomicU64 has the
        // layout of u64.
        unsafe { &*self.slots.base.cast::<AtomicU64>().add(i) }
    }

    /// The largest size a slot can hold.
    fn max_size(&self) -> usize {
        ((1usize << SIZE_BITS) - 1) * self.unit
    }

    fn pack(&self, p: *mut u8, size: usize) -> u64 {
        debug_assert!(size.is_multiple_of(self.unit) && size <= self.max_size());
        let addr = address(p) as u64;
        debug_assert!(addr & 7 == 0 && addr >> 3 < 1 << ADDR_BITS);
        (addr >> 3) | ((size / self.unit) as u64) << ADDR_BITS
    }

    fn unpack(&self, w: u64) -> (*mut u8, usize) {
        (block(((w & ((1 << ADDR_BITS) - 1)) << 3) as usize), (w >> ADDR_BITS) as usize * self.unit)
    }

    /// Puts a block into a random slot, freeing what it replaces.
    fn put(&self, me: &mut Holder, rng: &mut Rng, p: *mut u8, size: usize) {
        let i = rng.below(self.cap as u64) as usize;
        let old = self.slot(i).swap(self.pack(p, size), AcqRel);
        me.add(self, size as i64);
        if old != 0 {
            let (q, n) = self.unpack(old);
            free(q, n);
            me.add(self, -(n as i64));
        }
    }

    /// Takes a random entry out of the cache, if there is one near a random slot. Its bytes
    /// are still counted as the cache's; the caller counts them out.
    fn take(&self, rng: &mut Rng) -> Option<(*mut u8, usize)> {
        let start = rng.below(self.cap as u64) as usize;
        for k in 0..PROBE {
            let s = self.slot((start + k) % self.cap);
            if s.load(Relaxed) != 0 {
                let w = s.swap(0, AcqRel);
                if w != 0 {
                    return Some(self.unpack(w));
                }
            }
        }
        None
    }

    /// Evicts random entries while the cache is over its budget.
    fn trim(&self, me: &mut Holder, rng: &mut Rng) {
        let mut misses = 0;
        while me.estimate(self) > self.budget && misses < 1000 {
            match self.take(rng) {
                Some((p, n)) => {
                    free(p, n);
                    me.add(self, -(n as i64));
                }
                None => misses += 1,
            }
        }
    }

    /// Frees every entry; for the end of a run, when no other thread uses the cache.
    fn clear(&self) {
        for i in 0..self.cap {
            let w = self.slot(i).swap(0, Relaxed);
            if w != 0 {
                let (p, n) = self.unpack(w);
                free(p, n);
            }
        }
        self.live.0.store(0, Relaxed);
    }
}

/// One thread's accounts: what it has added to each cache's live bytes and not yet told the
/// cache, and the bytes it holds itself (its FIFO, its transients, values it has queued).
struct Holder {
    pending: [i64; 2],
    own: i64,
}

impl Holder {
    fn add(&mut self, cache: &Cache, n: i64) {
        let pending = &mut self.pending[cache.id];
        *pending += n;
        if pending.abs() > FLUSH {
            cache.live.0.fetch_add(*pending, Relaxed);
            *pending = 0;
        }
    }

    fn estimate(&self, cache: &Cache) -> i64 {
        cache.live.0.load(Relaxed) + self.pending[cache.id]
    }
}

/// A distribution of sizes: 4096 samples drawn once, so that drawing one is a table lookup.
struct Dist(Vec<u32>);

impl Dist {
    fn lognormal(median: f64, sigma: f64, max: usize, unit: usize, seed: u64) -> Dist {
        let mut rng = Rng::new(seed);
        let table = (0..4096)
            .map(|_| {
                let (u1, u2) = (rng.unit().max(1e-12), rng.unit());
                let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
                let s = (median * (sigma * z).exp()) as usize;
                s.clamp(16, max).next_multiple_of(unit) as u32
            })
            .collect();
        Dist(table)
    }

    fn loguniform(lo: usize, hi: usize, unit: usize, seed: u64) -> Dist {
        let mut rng = Rng::new(seed);
        let (a, b) = ((lo as f64).ln(), (hi as f64).ln());
        Dist((0..4096).map(|_| ((a + rng.unit() * (b - a)).exp() as usize).next_multiple_of(unit) as u32).collect())
    }

    fn sample(&self, rng: &mut Rng) -> usize {
        self.0[rng.below(self.0.len() as u64) as usize] as usize
    }

    fn mean(&self) -> f64 {
        self.0.iter().map(|&s| s as f64).sum::<f64>() / self.0.len() as f64
    }

    fn max(&self) -> usize {
        *self.0.iter().max().unwrap() as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    Steady,
    Shift,
    Lifetime,
    Swing,
    Large,
}

/// What one phase asks of the threads.
struct Phase {
    values: Dist,
    /// Threads that work in this phase.
    active: usize,
    /// `lifetime`: the bytes each thread's FIFO aims to hold.
    short_target: i64,
    label: String,
}

/// A FIFO of (address, size) in memory of the program's own.
struct Ring {
    map: Mapped,
    cap: usize,
    head: usize,
    len: usize,
}

impl Ring {
    fn new(cap: usize) -> Ring {
        Ring { map: Mapped::new(cap * 16), cap, head: 0, len: 0 }
    }

    fn at(&self, i: usize) -> *mut (usize, usize) {
        // SAFETY: `i % cap` is inside the mapping, which holds `cap` pairs.
        unsafe { self.map.base.cast::<(usize, usize)>().add(i % self.cap) }
    }

    fn push(&mut self, v: (usize, usize)) {
        assert!(self.len < self.cap, "ring full");
        // SAFETY: the slot is inside the mapping and not in use.
        unsafe { self.at(self.head + self.len).write(v) };
        self.len += 1;
    }

    fn pop(&mut self) -> Option<(usize, usize)> {
        if self.len == 0 {
            return None;
        }
        // SAFETY: the slot holds a value written by `push`.
        let v = unsafe { self.at(self.head).read() };
        self.head = (self.head + 1) % self.cap;
        self.len -= 1;
        Some(v)
    }

    fn is_full(&self) -> bool {
        self.len == self.cap
    }
}

/// Everything shared by the threads of a run.
struct Run {
    shape: Shape,
    threads: usize,
    samples: usize,
    ops: u64,
    seed: u64,
    phases: Vec<Phase>,
    transients: usize,
    transient_sizes: Dist,
    long_permille: u64,
    grow: u64,
    append: u64,
    /// `large`: one new buffer in a thousand of these is `big_factor` times the size drawn.
    big_permille: u64,
    big_factor: usize,
    /// The largest buffer `append` may grow to.
    append_max: usize,
    /// `lifetime`: entries each thread's FIFO has room for.
    fifo_cap: usize,
    main: &'static Cache,
    /// `lifetime`: the long-lived cache.
    long: Option<&'static Cache>,
    /// `swing`: the queue from producers to consumers, and how many producers are still
    /// producing this epoch.
    queue: crossbeam_queue::ArrayQueue<u64>,
    producing: AtomicUsize,
    /// What each thread holds itself, as of its last epoch.
    own: Vec<Line>,
    done: AtomicU64,
    /// The threads' own mappings (FIFOs, rings), for the sampler: (address, length).
    books: Mutex<Vec<(usize, usize)>>,
    barrier: Barrier,
    cpus: Cpus,
    pin: bool,
}

impl Run {
    fn phase_of(&self, epoch: usize) -> &Phase {
        &self.phases[epoch * self.phases.len() / self.samples]
    }

    fn caches(&self) -> impl Iterator<Item = &'static Cache> {
        [Some(self.main), self.long].into_iter().flatten()
    }

    /// The operations thread `tid` of `active` does in one epoch.
    fn share(&self, active: usize, tid: usize) -> u64 {
        let per_epoch = self.ops / self.samples as u64;
        per_epoch / active as u64 + u64::from((tid as u64) < per_epoch % active as u64)
    }

    /// The bytes the program holds: the caches' and what each thread holds itself. Exact
    /// while every thread is stopped at the barrier.
    fn live(&self) -> i64 {
        let caches: i64 = self.caches().map(|c| c.live.0.load(Relaxed)).sum();
        caches + self.own.iter().map(|l| l.0.load(Relaxed)).sum::<i64>()
    }

    /// Resident bytes of the program's own bookkeeping.
    fn books(&self) -> u64 {
        let caches: u64 = self.caches().map(|c| c.slots.resident()).sum();
        caches + self.books.lock().unwrap().iter().map(|&(base, len)| resident(base, len)).sum::<u64>()
    }
}

/// One thread's private state.
struct Worker {
    tid: usize,
    rng: Rng,
    me: Holder,
    ring: Ring,
    fifo: Ring,
    fifo_bytes: i64,
}

impl Worker {
    fn transients(&mut self, run: &Run) {
        for _ in 0..run.transients {
            let size = run.transient_sizes.sample(&mut self.rng);
            let p = make(size, 0x11);
            if self.ring.is_full() {
                let (q, n) = self.ring.pop().unwrap();
                free(block(q), n);
                self.me.own -= n as i64;
            }
            self.ring.push((address(p), size));
            self.me.own += size as i64;
        }
    }

    fn cache_op(&mut self, run: &Run, phase: &Phase) {
        let size = phase.values.sample(&mut self.rng);
        let p = make(size, self.tid as u8);
        run.main.put(&mut self.me, &mut self.rng, p, size);
        run.main.trim(&mut self.me, &mut self.rng);
    }

    fn lifetime_op(&mut self, run: &Run, phase: &Phase) {
        let size = phase.values.sample(&mut self.rng);
        let p = make(size, self.tid as u8);
        let long = run.long.expect("lifetime has a long-lived cache");
        if self.rng.below(1000) < run.long_permille {
            long.put(&mut self.me, &mut self.rng, p, size);
            long.trim(&mut self.me, &mut self.rng);
        } else {
            if self.fifo.is_full() {
                self.pop_short();
            }
            self.fifo.push((address(p), size));
            self.fifo_bytes += size as i64;
            self.me.own += size as i64;
        }
        // Over its target, a FIFO drains by one more than it gains, so a fall from the high
        // target to the low one is spread over the phase's first operations.
        for _ in 0..2 {
            if self.fifo_bytes > phase.short_target {
                self.pop_short();
            }
        }
    }

    fn pop_short(&mut self) {
        if let Some((q, n)) = self.fifo.pop() {
            free(block(q), n);
            self.fifo_bytes -= n as i64;
            self.me.own -= n as i64;
        }
    }

    fn large_op(&mut self, run: &Run, phase: &Phase) {
        let cache = run.main;
        let appended = self.rng.below(100) < run.append && self.append(run);
        if !appended {
            let mut target = phase.values.sample(&mut self.rng);
            if self.rng.below(1000) < run.big_permille {
                target = (target * run.big_factor).next_multiple_of(cache.unit).min(run.append_max);
            }
            let (p, size) =
                if self.rng.below(100) < run.grow { self.grown(run, target) } else { (make(target, 0x33), target) };
            cache.put(&mut self.me, &mut self.rng, p, size);
        }
        cache.trim(&mut self.me, &mut self.rng);
    }

    /// Grows a random cached buffer by half, as a buffer that is appended to; false if none
    /// was found.
    fn append(&mut self, run: &Run) -> bool {
        let cache = run.main;
        let Some((p, n)) = cache.take(&mut self.rng) else { return false };
        self.me.add(cache, -(n as i64));
        let new = (n + n / 2).next_multiple_of(cache.unit).min(run.append_max).max(n);
        let q = resize(p, n, new);
        cache.put(&mut self.me, &mut self.rng, q, new);
        true
    }

    /// A buffer built as a vector that is pushed to: doubling from `GROW_FROM` until it holds
    /// `target` bytes, and for half of them shrunk to fit.
    fn grown(&mut self, run: &Run, target: usize) -> (*mut u8, usize) {
        // Never past what a cache slot can record, which a power of two above the largest size
        // a slot holds would be.
        let most = run.main.max_size();
        let mut cur = GROW_FROM.min(target);
        let mut p = make(cur, 0x22);
        while cur < target {
            let next = (cur * 2).min(target.next_power_of_two()).min(most);
            p = resize(p, cur, next);
            cur = next;
        }
        if cur != target && self.rng.below(2) == 0 {
            p = resize(p, cur, target);
            cur = target;
        }
        (p, cur)
    }

    fn produce(&mut self, run: &Run, phase: &Phase) {
        let size = phase.values.sample(&mut self.rng);
        let p = make(size, self.tid as u8);
        self.me.own += size as i64;
        let mut w = run.main.pack(p, size);
        while let Err(back) = run.queue.push(w) {
            w = back;
            std::thread::yield_now();
        }
    }

    /// Takes values off the queue into the cache until every producer of the epoch is done
    /// and the queue is empty; returns how many it took.
    fn consume(&mut self, run: &Run) -> u64 {
        let mut took = 0;
        loop {
            match run.queue.pop() {
                Some(w) => {
                    let (p, n) = run.main.unpack(w);
                    self.me.own -= n as i64;
                    run.main.put(&mut self.me, &mut self.rng, p, n);
                    run.main.trim(&mut self.me, &mut self.rng);
                    self.transients(run);
                    took += 1;
                }
                None if run.producing.load(Acquire) == 0 && run.queue.is_empty() => return took,
                None => std::hint::spin_loop(),
            }
        }
    }

    /// One epoch's work for this thread; returns the operations it counts (a swing counts
    /// its operations where they end, in the cache).
    fn epoch(&mut self, run: &Run, epoch: usize) -> u64 {
        let phase = run.phase_of(epoch);
        let active = phase.active.min(run.threads);
        if self.tid >= active {
            return 0;
        }
        let op: fn(&mut Worker, &Run, &Phase) = match run.shape {
            Shape::Steady | Shape::Shift => Worker::cache_op,
            Shape::Lifetime => Worker::lifetime_op,
            Shape::Large => Worker::large_op,
            Shape::Swing if active == 1 => Worker::cache_op,
            Shape::Swing => {
                let producers = active / 2;
                if self.tid >= producers {
                    return self.consume(run);
                }
                for _ in 0..run.share(producers, self.tid) {
                    self.produce(run, phase);
                    self.transients(run);
                }
                run.producing.fetch_sub(1, Release);
                return 0;
            }
        };
        let ops = run.share(active, self.tid);
        for _ in 0..ops {
            op(self, run, phase);
            self.transients(run);
        }
        ops
    }

    /// Tells the caches and the run what this thread holds, so that a sample is exact.
    fn settle(&mut self, run: &Run) {
        for cache in run.caches() {
            cache.live.0.fetch_add(std::mem::take(&mut self.me.pending[cache.id]), Relaxed);
        }
        run.own[self.tid].0.store(self.me.own, Relaxed);
    }
}

fn status_field(text: &str, key: &str) -> u64 {
    text.lines()
        .find_map(|l| l.strip_prefix(key))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        .unwrap_or(0)
        * 1024
}

fn minor_faults() -> u64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(7)).and_then(|f| f.parse().ok()).unwrap_or(0)
}

/// `memory.current` of this process's cgroup (a run under `scripts/memlimit` has one of its
/// own): the memory the kernel charges, which counts a huge page that is partly unmapped
/// whole until the kernel splits it, where the resident set counts only the pages still
/// mapped. Zero where it cannot be read.
fn cgroup_current() -> u64 {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    let Some(path) = cgroup.lines().find_map(|l| l.strip_prefix("0::")) else { return 0 };
    std::fs::read_to_string(format!("/sys/fs/cgroup{}/memory.current", path.trim()))
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

struct Sampler {
    start: Instant,
    /// Breakdowns taken so far.
    accounts: u64,
    account_every: usize,
}

impl Sampler {
    fn sample(&mut self, run: &Run, kind: &str, epoch: usize, phase: &str, account: bool) -> f64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let rollup = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
        let anon = status_field(&status, "RssAnon:");
        let book = run.books();
        let heap = anon.saturating_sub(book);
        let live = run.live();
        let ratio = heap as f64 / live.max(1) as f64;
        let tag = if account && bench_alloc::snapshot_supported() {
            self.accounts += 1;
            bench_alloc::snapshot(self.accounts);
            self.accounts as i64
        } else {
            -1
        };
        println!(
            "DRIFT {{\"alloc\":\"{}\",\"kind\":\"{kind}\",\"epoch\":{epoch},\"phase\":\"{phase}\",\"ops\":{},\"t_ms\":{},\"live\":{live},\"rss_anon\":{anon},\"book\":{book},\"heap\":{heap},\"ratio\":{ratio:.5},\"thp\":{},\"pte\":{},\"minflt\":{},\"cgroup\":{},\"account\":{tag}}}",
            bench_alloc::NAME,
            run.done.load(Relaxed),
            self.start.elapsed().as_millis(),
            status_field(&rollup, "AnonHugePages:"),
            status_field(&status, "VmPTE:"),
            minor_faults(),
            cgroup_current(),
        );
        ratio
    }

    fn wants_account(&self, epoch: usize, samples: usize) -> bool {
        self.account_every > 0 && ((epoch + 1).is_multiple_of(self.account_every) || epoch + 1 == samples)
    }
}

fn parse_list(s: &str) -> Vec<u64> {
    s.split(',').map(|v| bench_harness::parse_num(v).unwrap_or_else(|| panic!("{v} is not a number"))).collect()
}

fn parse_ranges(s: &str) -> Vec<(usize, usize)> {
    s.split(',')
        .map(|r| {
            let (a, b) = r.split_once('-').unwrap_or_else(|| panic!("range {r} is not LO-HI"));
            let n = |v: &str| bench_harness::parse_num(v).unwrap_or_else(|| panic!("{v} is not a number")) as usize;
            (n(a), n(b))
        })
        .collect()
}

fn phases(args: &Args, shape: Shape, threads: usize, unit: usize, seed: u64, samples: usize) -> Vec<Phase> {
    let default_phases = match shape {
        Shape::Steady => 1,
        Shape::Large => 8,
        _ => 12,
    };
    let count = args.usize("phases", default_phases).clamp(1, samples);
    let sigma = args.str("sigma", "1.0").parse::<f64>().expect("sigma is a number");
    let max_value = args.usize("max", 64 << 10);
    let medians = match shape {
        Shape::Shift => parse_list(&args.str("medians", "128,384,1024")),
        _ => vec![args.num("median", 128)],
    };
    let actives = match shape {
        Shape::Swing => parse_list(&args.str("active", "8,2,8,1,8,4")),
        _ => vec![threads as u64],
    };
    let ranges = parse_ranges(&args.str("ranges", "40Ki-256Ki,256Ki-4Mi,4Mi-32Mi,40Ki-32Mi"));
    let (short_hi, short_lo) = (args.usize("short_hi", 1536 << 20), args.usize("short_lo", 128 << 20));
    // Phases that ask for the same sizes draw the same table, so that a class's share of the
    // values is the same each time; `tables=phase` gives every phase a table of its own, whose
    // classes' shares differ by the sampling noise of 4096 draws (tens of percent for a class
    // that takes one draw in a hundred), as the runs of 2026-10-24 did.
    let per_phase = args.str("tables", "shared") == "phase";
    let sizes_of = |i: usize| match shape {
        Shape::Large => i % ranges.len(),
        _ => i % medians.len(),
    };
    (0..count)
        .map(|i| {
            let dist_seed = seed * 1000 + if per_phase { i } else { sizes_of(i) } as u64;
            let (values, sizes) = match shape {
                Shape::Large => {
                    let (lo, hi) = ranges[i % ranges.len()];
                    (Dist::loguniform(lo, hi, unit, dist_seed), format!("{lo}-{hi}"))
                }
                _ => {
                    let m = medians[i % medians.len()];
                    (Dist::lognormal(m as f64, sigma, max_value, unit, dist_seed), format!("m{m}"))
                }
            };
            let active = (actives[i % actives.len()] as usize).clamp(1, threads);
            let high = i % 2 == 0;
            let label = match shape {
                Shape::Swing => format!("t{active}"),
                Shape::Lifetime => format!("{}-{sizes}", if high { "hi" } else { "lo" }),
                _ => sizes,
            };
            let short = if high { short_hi } else { short_lo };
            Phase { values, active, short_target: (short / threads) as i64, label }
        })
        .collect()
}

pub fn drift(args: &Args) {
    let shape = match args.str("shape", "steady").as_str() {
        "steady" => Shape::Steady,
        "shift" => Shape::Shift,
        "lifetime" => Shape::Lifetime,
        "swing" => Shape::Swing,
        "large" => Shape::Large,
        s => panic!("shape={s} is not steady, shift, lifetime, swing or large"),
    };
    let cpus = Cpus::from_env();
    let threads = args.usize("threads", cpus.len().min(8));
    let samples = args.usize("samples", 120);
    let ops = args.num("ops", 100_000_000);
    let budget = args.usize("live", 2 << 30);
    let seed = args.num("seed", 1);
    let unit = if shape == Shape::Large { 64 } else { 8 };
    let phases = phases(args, shape, threads, unit, seed, samples);
    // Room for twice the entries a budget holds at the smallest mean size, so that a random
    // slot is empty about half the time or more.
    let smallest_mean = phases.iter().map(|p| p.values.mean()).fold(f64::MAX, f64::min);
    let slots = |bytes: usize| (2.0 * bytes as f64 / smallest_mean) as usize;
    let main: &'static Cache = Box::leak(Box::new(Cache::new(0, budget, slots(budget), unit)));
    let long_bytes = args.usize("long", 384 << 20);
    let long =
        (shape == Shape::Lifetime).then(|| &*Box::leak(Box::new(Cache::new(1, long_bytes, slots(long_bytes), unit))));
    let largest = phases.iter().map(|p| p.values.max()).max().unwrap();
    assert!(largest <= main.max_size(), "sizes up to {largest} do not fit a slot");
    // Twice the entries of the mean size that the largest target holds. The ring wraps round
    // all of it in a long run, so it is not sized for the smallest values; a FIFO that fills
    // early lets its oldest entry go.
    let fifo_cap = match shape {
        Shape::Lifetime => {
            let target = phases.iter().map(|p| p.short_target).max().unwrap() as f64;
            (2.0 * target / smallest_mean) as usize + 1024
        }
        _ => 1,
    };
    let run = Run {
        shape,
        threads,
        samples,
        ops,
        seed,
        phases,
        transients: args.usize("transients", 2),
        transient_sizes: Dist::lognormal(96.0, 1.2, 8192, 8, seed ^ 0x7e),
        long_permille: args.num("long_permille", 20),
        grow: args.num("grow", 50),
        append: args.num("append", 20),
        big_permille: args.num("big_permille", 0),
        big_factor: args.usize("big_factor", 10),
        append_max: (largest * 2).min(main.max_size()),
        fifo_cap,
        main,
        long,
        queue: crossbeam_queue::ArrayQueue::new(1 << 16),
        producing: AtomicUsize::new(0),
        own: (0..threads).map(|_| Line::default()).collect(),
        done: AtomicU64::new(0),
        books: Mutex::new(Vec::new()),
        barrier: Barrier::new(threads),
        cpus,
        pin: args.str("pin", "1") != "0",
    };
    let mut sampler = Sampler { start: Instant::now(), accounts: 0, account_every: args.usize("account_every", 0) };
    let counters = Counters::region();
    let measure = Measure::start(&counters);
    let mut ratios = Vec::with_capacity(samples);
    std::thread::scope(|scope| {
        let run = &run;
        let handles: Vec<_> = (1..threads).map(|tid| scope.spawn(move || work(run, tid, None))).collect();
        work(run, 0, Some((&mut sampler, &mut ratios)));
        handles.into_iter().for_each(|h| h.join().unwrap());
    });
    let load = measure.stop();
    // Quiet: the live set stays and nothing allocates.
    let quiet = Duration::from_secs(args.num("quiet", 60));
    let step = Duration::from_secs(args.num("quiet_step", 5)).max(Duration::from_millis(100));
    let begin = Instant::now();
    let mut quiet_ratio = 0.0;
    while begin.elapsed() < quiet {
        std::thread::sleep(step.min(quiet.saturating_sub(begin.elapsed())));
        let last = begin.elapsed() >= quiet;
        quiet_ratio = sampler.sample(&run, "quiet", samples, "quiet", last && sampler.account_every > 0);
    }
    let total = run.done.load(Relaxed);
    let cycles = load.counters.get("cycles").unwrap_or(0) as f64;
    let n = ratios.len();
    // The trend at the end: the mean of the last quarter's samples against the quarter's
    // before it, which a cycle of phases that divides the run into quarters visits alike.
    let quarter = (n / 4).max(1);
    let mean = |s: &[f64]| s.iter().sum::<f64>() / s.len().max(1) as f64;
    Report::new("drift", args)
        .phase("load", &load)
        .param("threads", threads)
        .param("samples", samples)
        .param("table_slots", main.cap)
        .metric("ops", total)
        .metric("ratio_end", ratios.last().copied().unwrap_or(0.0))
        .metric("ratio_max", ratios.iter().copied().fold(0.0, f64::max))
        .metric("ratio_last_quarter", mean(&ratios[n.saturating_sub(quarter)..]))
        .metric("ratio_quarter_before", mean(&ratios[n.saturating_sub(2 * quarter)..n.saturating_sub(quarter)]))
        .metric("ratio_quiet", quiet_ratio)
        .primary("cycles/op", cycles / total.max(1) as f64, Better::Lower)
        .emit();
    main.clear();
    if let Some(l) = long {
        l.clear();
    }
}

fn work(run: &Run, tid: usize, mut sampler: Option<(&mut Sampler, &mut Vec<f64>)>) {
    if run.pin {
        run.cpus.pin(tid);
    }
    let mut w = Worker {
        tid,
        rng: Rng::new(run.seed.wrapping_mul(0x9e37_79b9) ^ ((tid as u64 + 1) * 7919)),
        me: Holder { pending: [0; 2], own: 0 },
        ring: Ring::new(RING),
        fifo: Ring::new(run.fifo_cap),
        fifo_bytes: 0,
    };
    run.books.lock().unwrap().extend([&w.ring, &w.fifo].map(|r| (address(r.map.base), r.map.len)));
    for epoch in 0..run.samples {
        if tid == 0 && run.shape == Shape::Swing {
            let active = run.phase_of(epoch).active;
            run.producing.store(if active > 1 { active / 2 } else { 0 }, Release);
        }
        run.barrier.wait();
        let done = w.epoch(run, epoch);
        run.done.fetch_add(done, Relaxed);
        w.settle(run);
        run.barrier.wait();
        if let Some((s, ratios)) = sampler.as_mut() {
            let account = s.wants_account(epoch, run.samples);
            ratios.push(s.sample(run, "load", epoch, &run.phase_of(epoch).label, account));
        }
    }
    // The thread's transients and FIFO stay live through the quiet phase and are freed by
    // the process's exit; their mappings stay, so the sampler can still read them.
    std::mem::forget(w);
}
