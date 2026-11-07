//! Real threads on CPUs far apart, for the tests that look for a missing memory order. Test code
//! only.
//!
//! The interleaving checkers run one thread at a time, so no store is ever late, and a passing run
//! on x86-64 says nothing about a store and a load that swap. These tests run a protocol's
//! production code on threads that go round the CPUs the process may use, and check what must hold
//! whatever the hardware does. A pass on one CPU does not show that an order is right, since
//! another CPU may reorder what this one does not: the argument is
//! `docs/research/memory-ordering.md`, and these are what catches a mistake in it.
//!
//! Two shapes of test:
//!
//! - **Rounds** ([`Rounds`], [`Gate`]): a few threads meet, each does its one step at about the
//!   same moment, and the outcome is checked. That is a litmus test on the production code, as
//!   for a pattern of a store and then a load on both sides.
//! - **Ledgers** ([`own`]): blocks that go from owner to owner through the allocator carry two
//!   counters on lines apart, written in order by each owner. An owner that does not see both of
//!   the last owner's writes has no happens-before edge from it, whichever way the hand-over went.
//!
//! A run lasts [`QUICK`] for the gate, and [`FULL`] for the `_fully` tests. Under Miri, whose weak
//! memory emulation finds in a few rounds what hardware takes millions to show, a test is a few
//! rounds on a timer that does not matter. `CRALLOC_STRESS_MS` overrides the time of either.

use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::time::{Duration, Instant};
use std::vec::Vec;

/// How long a test in the gate runs.
pub(crate) const QUICK: Duration = Duration::from_millis(300);
/// How long a test run on demand runs (`scripts/gate --all`).
pub(crate) const FULL: Duration = Duration::from_secs(20);

/// The CPUs this process may run on, lowest first.
#[cfg(all(target_os = "linux", not(miri)))]
fn allowed_cpus() -> Vec<usize> {
    unsafe extern "C" {
        fn sched_getaffinity(pid: i32, size: usize, mask: *mut u64) -> i32;
    }
    let mut mask = [0u64; 64];
    // SAFETY: the call writes at most `size_of_val(&mask)` bytes to `mask`, a live local.
    let got = unsafe { sched_getaffinity(0, size_of_val(&mask), mask.as_mut_ptr()) };
    if got != 0 {
        return Vec::new();
    }
    (0..64 * 64)
        .filter(|&cpu| mask[cpu / 64] & (1 << (cpu % 64)) != 0)
        .collect()
}

/// Pins the calling thread to `cpu`, if that can be done.
#[cfg(all(target_os = "linux", not(miri)))]
fn pin(cpu: usize) {
    unsafe extern "C" {
        fn sched_setaffinity(pid: i32, size: usize, mask: *const u64) -> i32;
    }
    let mut mask = [0u64; 64];
    mask[cpu / 64] |= 1 << (cpu % 64);
    // SAFETY: the call reads `size_of_val(&mask)` bytes from `mask`, a live local. A refusal
    // leaves the thread where it is, which costs the test its reach and nothing else.
    let _ = unsafe { sched_setaffinity(0, size_of_val(&mask), mask.as_ptr()) };
}

/// How long a thread of [`spread`] stays on a CPU. Short enough that a test in the gate visits
/// every pair of CPUs on a small machine.
#[cfg(all(target_os = "linux", not(miri)))]
const STAY: Duration = Duration::from_millis(40);

/// Which CPUs a thread goes round, for [`Rounds::go_on`] to move it. Only the hardware says which
/// pairs of CPUs show a reordering: on an M1 Max, none whose writer is an efficiency core, and many
/// more between two performance cores of a cluster than across clusters
/// (`docs/research/memory-ordering.md`). So every test visits all of them.
#[cfg(all(target_os = "linux", not(miri)))]
struct Tour {
    cpus: Vec<usize>,
    /// Where the thread's CPU is in `cpus`.
    at: core::cell::Cell<usize>,
    moved: core::cell::Cell<Instant>,
}

#[cfg(all(target_os = "linux", not(miri)))]
std::thread_local! {
    static TOUR: core::cell::RefCell<Option<Tour>> = const { core::cell::RefCell::new(None) };
}

/// Moves the calling thread on to its tour's next CPU if it has stayed long enough.
#[cfg(all(target_os = "linux", not(miri)))]
fn move_on(now: Instant) {
    TOUR.with_borrow(|tour| {
        if let Some(tour) = tour
            && now.duration_since(tour.moved.get()) >= STAY
        {
            let at = (tour.at.get() + 1) % tour.cpus.len();
            pin(tour.cpus[at]);
            tour.at.set(at);
            tour.moved.set(now);
        }
    });
}

/// Starts `threads` threads, the `i`-th running `body(i)`, at `first(i, cpus)` in the CPUs the
/// process may use, and returns their results in order. `tour` says whether they go round the CPUs
/// from there. With fewer CPUs than threads, or under Miri, they run where they fall.
fn run<R: Send>(
    threads: usize,
    first: impl Fn(usize, usize) -> usize,
    tour: bool,
    body: impl Fn(usize) -> R + Sync,
) -> Vec<R> {
    #[cfg(any(not(target_os = "linux"), miri))]
    let _ = (first, tour);
    #[cfg(all(target_os = "linux", not(miri)))]
    let cpus = allowed_cpus();
    let body = &body;
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                #[cfg(all(target_os = "linux", not(miri)))]
                let start = (cpus.len() >= threads).then(|| first(i, cpus.len()) % cpus.len());
                #[cfg(all(target_os = "linux", not(miri)))]
                let cpus = cpus.clone();
                scope.spawn(move || {
                    let _watch = Watch;
                    #[cfg(all(target_os = "linux", not(miri)))]
                    if let Some(start) = start {
                        pin(cpus[start]);
                        if tour {
                            TOUR.set(Some(Tour {
                                cpus,
                                at: start.into(),
                                moved: Instant::now().into(),
                            }));
                        }
                    }
                    body(i)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

/// Whether a thread of these tests has panicked, so that the threads waiting for it stop waiting and
/// the test reports the panic, where it would otherwise hang until its time limit. One flag for the
/// process, since a failure is the end of the run anyway.
static FAILED: AtomicBool = AtomicBool::new(false);

/// Whether a thread of these tests has panicked: for a test's own waits.
pub(crate) fn failed() -> bool {
    FAILED.load(Relaxed)
}

/// Sets [`FAILED`] if the thread that holds it panics.
struct Watch;

impl Drop for Watch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            FAILED.store(true, Relaxed);
        }
    }
}

/// Which CPU of the list the next run of threads starts from, so that a test that runs threads many
/// times visits them all too.
fn next_start() -> usize {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    RUNS.fetch_add(1, Relaxed)
}

/// Runs `body` on `threads` threads, spread evenly over the CPUs the process may use, each moving
/// on to the next CPU as it runs [`Rounds::go_on`]. Returns each thread's result, in order.
pub(crate) fn spread<R: Send>(threads: usize, body: impl Fn(usize) -> R + Sync) -> Vec<R> {
    let start = next_start();
    run(threads, |i, cpus| start + i * cpus / threads, true, body)
}

/// As [`spread`], with the threads staying where they start: for two threads on the first and
/// the middle of the CPUs, which a fence can name.
#[cfg(all(target_os = "linux", not(miri)))]
pub(crate) fn spread_still<R: Send>(threads: usize, body: impl Fn(usize) -> R + Sync) -> Vec<R> {
    run(threads, |i, cpus| i * cpus / threads, false, body)
}

/// As [`spread`], with the threads on one CPU and moving together, near enough: what passes
/// between them passes through context switches.
pub(crate) fn together<R: Send>(threads: usize, body: impl Fn(usize) -> R + Sync) -> Vec<R> {
    let start = next_start();
    run(threads, |_, _| start, true, body)
}

/// When a test's threads stop: the time, or under Miri a few rounds.
#[derive(Clone, Copy)]
pub(crate) struct Rounds {
    until: Instant,
    /// How many rounds a thread takes under Miri.
    #[cfg(miri)]
    most: u64,
}

impl Rounds {
    /// A run of `time`, or of `miri` rounds under Miri.
    pub(crate) fn of(time: Duration, miri: u64) -> Rounds {
        let time = std::env::var("CRALLOC_STRESS_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .map_or(time, Duration::from_millis);
        let _ = miri;
        Rounds {
            until: Instant::now() + time,
            #[cfg(miri)]
            most: miri,
        }
    }

    /// Whether round `round` of a thread, counted from zero, is to be run. Moves a thread of
    /// [`spread`] on to its next CPU when it is time.
    pub(crate) fn go_on(&self, round: u64) -> bool {
        #[cfg(miri)]
        return round < self.most && !failed();
        #[cfg(not(miri))]
        {
            // The clock once in a while: it is a system call on some machines.
            if !round.is_multiple_of(64) {
                return true;
            }
            let now = Instant::now();
            #[cfg(target_os = "linux")]
            move_on(now);
            now < self.until && !failed()
        }
    }
}

/// A spin barrier for a few threads, so that they act within a few cycles of each other. A round's
/// outcome is read after the next [`Gate::pass`], when nothing is running.
pub(crate) struct Gate {
    parties: usize,
    arrived: AtomicUsize,
    turn: AtomicU64,
}

impl Gate {
    pub(crate) const fn new(parties: usize) -> Gate {
        Gate {
            parties,
            arrived: AtomicUsize::new(0),
            turn: AtomicU64::new(0),
        }
    }

    /// Waits for the other parties. Every access before it on any thread happens before every
    /// access after it on any thread.
    pub(crate) fn pass(&self) {
        let turn = self.turn.load(Acquire);
        if self.arrived.fetch_add(1, AcqRel) + 1 == self.parties {
            self.arrived.store(0, Relaxed);
            self.turn.store(turn + 1, Release);
        } else {
            while self.turn.load(Acquire) == turn {
                assert!(!failed(), "another thread of this test failed");
                core::hint::spin_loop();
            }
        }
    }
}

/// A number that differs from round to round, to spin for, so that two threads' steps slide past
/// each other over a run instead of keeping one distance.
pub(crate) fn jitter(state: &mut u64) {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    for _ in 0..*state % 16 {
        core::hint::spin_loop();
    }
}

/// The distance between a ledger's two counters: lines apart on any CPU, so that they can be
/// written back at different times.
const APART: usize = 256;

/// The bytes a block needs to hold a ledger.
pub(crate) const LEDGER_BYTES: usize = APART + 8;

/// The ledger's two counters, in a block of at least [`LEDGER_BYTES`].
///
/// # Safety
///
/// `block` is valid for reads and writes of [`LEDGER_BYTES`], aligned to 8, and accessed only
/// atomically while it is in use.
unsafe fn ledger<'a>(block: *mut u8) -> (&'a AtomicU64, &'a AtomicU64) {
    // SAFETY: the caller's promise.
    unsafe {
        (
            AtomicU64::from_ptr(block.cast()),
            AtomicU64::from_ptr(block.add(APART).cast()),
        )
    }
}

/// Takes a block's ledger as its new owner: checks that it reads what one owner wrote, and adds
/// one, the first counter and then the second. Relaxed, so that nothing here supplies the order
/// the allocator's hand-over must. Returns the count it wrote.
///
/// # Safety
///
/// As for [`ledger`], and the caller owns the block.
pub(crate) unsafe fn own(block: *mut u8) -> u64 {
    // SAFETY: the caller's promise.
    let (first, second) = unsafe { ledger(block) };
    // The second first: the last owner wrote the first first, so if it were late the two differ.
    let high = second.load(Relaxed);
    let low = first.load(Relaxed);
    assert_eq!(
        low, high,
        "a block's new owner did not see all of what its last owner wrote"
    );
    first.store(low + 1, Relaxed);
    second.store(low + 1, Relaxed);
    low + 1
}

/// What marks a word of a ledger written by [`own_shared`], in its high half.
const SHARED: u64 = 0x5ca1_ab1e << 32;

/// As [`own`], on a heap other tests of the process use too, which leave what they like in a block:
/// the check is only of two words that both say they are a ledger's. So a block's first ledger, and
/// one after a purge, are not checked.
///
/// # Safety
///
/// As for [`own`].
pub(crate) unsafe fn own_shared(block: *mut u8) {
    // SAFETY: the caller's promise.
    let (first, second) = unsafe { ledger(block) };
    let high = second.load(Relaxed);
    let low = first.load(Relaxed);
    let marked = |word: u64| word & !(u32::MAX as u64) == SHARED;
    let last = if marked(low) && marked(high) {
        assert_eq!(
            low, high,
            "a block's new owner did not see all of what its last owner wrote"
        );
        low
    } else {
        SHARED
    };
    first.store(last + 1, Relaxed);
    second.store(last + 1, Relaxed);
}

/// The count in a block's ledger, once nothing else touches it.
///
/// # Safety
///
/// As for [`ledger`].
pub(crate) unsafe fn count(block: *mut u8) -> u64 {
    // SAFETY: the caller's promise.
    unsafe { ledger(block) }.0.load(Relaxed)
}
