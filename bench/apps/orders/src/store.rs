//! The order service the axum and actix-web servers share: POST /orders parses an order, prices
//! it, keeps it in a sharded in-memory store that holds the most recent `keep` orders, and
//! answers with the stored order as JSON. Old orders are dropped by whichever worker
//! inserts the order that displaces them, so blocks cross threads as they do in a cache.
//!
//! A shard's table grows by doubling, rehashing every entry while the shard's lock is held,
//! and with inserts and removes churning it, a table more than half full doubles again once
//! the churn has used up its room: one stall per shard, all shards within seconds, long after
//! the store reached `keep` (docs/research/servers-large.md, "The 4 GiB tail"). `presize`
//! gives each shard room for twice its share from the start, so that it never doubles. Every
//! rehash is timed, with the page faults it took, for the job that measures the doubling.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// How many orders the store holds by default (about 200 MiB resident).
pub const KEEP: u64 = 100_000;
const SHARDS: usize = 64;

#[derive(Deserialize, Serialize, Clone)]
pub struct Item {
    pub sku: String,
    pub qty: u32,
    pub price_cents: u64,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
pub struct NewOrder {
    pub customer: String,
    pub items: Vec<Item>,
    pub note: Option<String>,
}

#[derive(Serialize)]
pub struct Order {
    pub id: u64,
    pub customer: String,
    pub items: Vec<Item>,
    pub total_cents: u64,
    pub note: Option<String>,
    pub summary: String,
}

struct Shard {
    map: HashMap<u64, Arc<Order>>,
    /// The table's capacity after its last rehash: a rehash that ends above it grew the table.
    full: usize,
}

pub struct Store {
    shards: Vec<Mutex<Shard>>,
    next: AtomicU64,
    keep: u64,
}

impl Store {
    /// A store that holds the most recent `keep` orders, its shards sized for twice their
    /// share of them if `presize`.
    pub fn new(keep: u64, presize: bool) -> Self {
        let room = if presize { (2 * keep as usize).div_ceil(SHARDS) } else { 0 };
        let shard = || {
            let map = HashMap::with_capacity(room);
            Mutex::new(Shard { full: map.capacity(), map })
        };
        Store { shards: (0..SHARDS).map(|_| shard()).collect(), next: AtomicU64::new(0), keep }
    }
}

impl Default for Store {
    fn default() -> Self {
        Store::new(KEEP, false)
    }
}

/// One rehash of a shard's table, as its insert saw it.
#[derive(Clone, Copy)]
pub struct Rehash {
    /// Whether the table grew (a doubling) rather than rehashing in place.
    pub grew: bool,
    /// The table's capacity after it.
    pub capacity: usize,
    pub ns: u64,
    /// Minor page faults the inserting thread took during it.
    pub faults: u64,
    /// When it started, CLOCK_MONOTONIC ns, and on which thread: to line it up with traces.
    pub start_ns: u64,
    pub tid: u64,
    /// The inserting thread's CPU time during it: the rest of `ns` it spent off its CPU.
    pub cpu_ns: u64,
    /// Voluntary and involuntary context switches the thread took during it.
    pub vcsw: u64,
    pub ivcsw: u64,
    /// The cycle counter at its start, to line it up with an allocator's own timed events.
    pub tsc: u64,
}

static REHASHES: Mutex<Vec<Rehash>> = Mutex::new(Vec::new());

/// Every rehash so far.
pub fn rehashes() -> Vec<Rehash> {
    REHASHES.lock().unwrap().clone()
}

/// The calling thread's minor faults, voluntary and involuntary context switches.
fn thread_usage() -> (u64, u64, u64) {
    let mut u = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage writes one rusage.
    if unsafe { libc::getrusage(libc::RUSAGE_THREAD, u.as_mut_ptr()) } != 0 {
        return (0, 0, 0);
    }
    // SAFETY: initialised by the successful call.
    let u = unsafe { u.assume_init() };
    (u.ru_minflt as u64, u.ru_nvcsw as u64, u.ru_nivcsw as u64)
}

fn clock_ns(clock: libc::clockid_t) -> u64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes one timespec.
    unsafe { libc::clock_gettime(clock, &mut t) };
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

impl Shard {
    fn insert(&mut self, id: u64, order: Arc<Order>) {
        // A table rehashes only on an insert that finds no room left; only then is it timed.
        if self.map.len() < self.map.capacity() {
            self.map.insert(id, order);
            return;
        }
        // SAFETY: rdtsc has no preconditions.
        let tsc = unsafe { core::arch::x86_64::_rdtsc() };
        let (start_ns, cpu) = (clock_ns(libc::CLOCK_MONOTONIC), clock_ns(libc::CLOCK_THREAD_CPUTIME_ID));
        let (t, (f, v, i)) = (Instant::now(), thread_usage());
        self.map.insert(id, order);
        let ns = t.elapsed().as_nanos() as u64;
        let cpu_ns = clock_ns(libc::CLOCK_THREAD_CPUTIME_ID) - cpu;
        let (f2, v2, i2) = thread_usage();
        let (faults, vcsw, ivcsw) = (f2 - f, v2 - v, i2 - i);
        // SAFETY: gettid has no preconditions.
        let tid = unsafe { libc::gettid() } as u64;
        let capacity = self.map.capacity();
        if capacity == self.map.len() {
            return; // the insert took a deleted slot, with no rehash
        }
        let grew = capacity > self.full;
        self.full = capacity;
        REHASHES.lock().unwrap().push(Rehash { grew, capacity, ns, faults, start_ns, tid, cpu_ns, vcsw, ivcsw, tsc });
    }
}

impl Store {
    pub fn create(&self, new: NewOrder) -> String {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let total_cents = new.items.iter().map(|i| i.price_cents * i.qty as u64).sum();
        let mut skus: Vec<&str> = new.items.iter().map(|i| i.sku.as_str()).collect();
        skus.sort_unstable();
        let summary = format!("{} items for {}: {}", new.items.len(), new.customer, skus.join(","));
        let order =
            Arc::new(Order { id, customer: new.customer, items: new.items, total_cents, note: new.note, summary });
        let body = serde_json::to_string(&*order).expect("serialise");
        self.shards[id as usize % SHARDS].lock().unwrap().insert(id, order);
        if id >= self.keep {
            let old = id - self.keep;
            let gone = self.shards[old as usize % SHARDS].lock().unwrap().map.remove(&old);
            drop(gone);
        }
        body
    }
}
