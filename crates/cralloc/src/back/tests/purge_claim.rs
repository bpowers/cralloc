//! What a fill pays for words a purge holds. A fill must not wait for a purge (design: "Concurrency
//! rules"), and the words of a large huge page of small slots are far too many to walk on a
//! request's path, however cheaply each is claimed. So a purge clears their summary bits as it
//! claims them, and a fill passes over what it holds.

use super::super::testhost::{TestHost, TestWord};
use super::super::tree::{self, ExactRun, Host};
use super::super::word::{Atomic, BITS, Word, bits_in_word};
use crate::classes::Class;
use crate::os::sim::{PAGE_SCALE, Shape, Standard};
use core::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// Operations on every [`Counted`] word so far.
static OPERATIONS: AtomicUsize = AtomicUsize::new(0);

/// A word that counts what is done to it, which measures a walk whatever the machine.
struct Counted(Atomic);

impl Word for Counted {
    fn load(&self) -> u64 {
        OPERATIONS.fetch_add(1, Relaxed);
        self.0.load()
    }

    fn fetch_or(&self, bits: u64) -> u64 {
        OPERATIONS.fetch_add(1, Relaxed);
        self.0.fetch_or(bits)
    }

    fn fetch_and(&self, bits: u64) -> u64 {
        OPERATIONS.fetch_add(1, Relaxed);
        self.0.fetch_and(bits)
    }

    fn compare_exchange(&self, current: u64, new: u64) -> Result<u64, u64> {
        OPERATIONS.fetch_add(1, Relaxed);
        self.0.compare_exchange(current, new)
    }

    fn store(&self, value: u64) {
        OPERATIONS.fetch_add(1, Relaxed);
        self.0.store(value)
    }
}

impl TestWord for Counted {
    fn new(_: Option<u64>) -> Counted {
        Counted(Atomic::zero())
    }
}

/// The operations a fill's first claim makes in `host`, where it must find nothing.
fn fill_finding_nothing(host: &TestHost<Counted>) -> usize {
    let before = OPERATIONS.load(Relaxed);
    let mut cursor = None;
    assert!(tree::claim_next(host, 64, &mut cursor, &mut |_| {}).is_none());
    OPERATIONS.load(Relaxed) - before
}

/// Everything in the region free but for what a purge holds, so the fill has nowhere else to go.
/// A claim that left the summary bits set had a fill clear one bit and then walk every group, for
/// each word: tens of thousands of operations here, and over a thousand million in a huge page of
/// 8-byte slots of 32 MiB, for as long as the purge held.
#[test]
fn a_fill_does_not_walk_the_words_a_purge_holds() {
    let host = TestHost::<Counted>::new(
        Standard::GEOMETRY.largest_plan(),
        Class::of_size(8 * PAGE_SCALE).unwrap(),
        1,
    );
    let r0 = host.region_at(0);
    let slots = host.layout().slots();
    host.carve_all(r0, slots);
    let words = host.layout().leaf_words();
    assert!(words > 4 * BITS, "more than a group or two");
    let bits = |w| bits_in_word(w, 0, slots - 1);
    for w in 0..words {
        tree::release(&host, r0, w, bits(w));
    }

    let mut claims = ExactRun::new();
    for w in 0..words {
        assert!(claims.claim(&host, r0, w, bits(w)));
    }
    // While the last group's bits are still to clear: no more than a group's words, each a look
    // from the top and a clear, whatever the size of the unit.
    let under_way = fill_finding_nothing(&host);
    assert!(under_way <= BITS as usize * 16, "{under_way} operations");

    // Once the purge has its claim: nothing to find, a walk down from the top at most.
    claims.finish(&host);
    let held = fill_finding_nothing(&host);
    assert!(held <= 16, "{held} operations");
}

/// How long a fill's first claim takes beside a purge, in a region with nothing else free: while
/// the purge finishes claiming, once it holds its claim, and, for what `docs/research/memory.md`
/// says of it, had the purge left the summary bits set. It prints, and passes whatever it finds:
/// read it from the test binary pinned to a free CPU, under `scripts/measure`.
#[cfg(not(miri))]
#[test]
#[ignore = "slow: prints timings, which mean something only under scripts/measure"]
fn what_a_fill_takes_beside_a_purge() {
    use crate::classes::MagazineSize;
    use crate::space::Geometry;
    use std::time::Instant;
    const WIDE: Geometry = Geometry::new(14, 25, 26, 26, 8, 1, MagazineSize::MAX);
    let fill = |host: &TestHost<Atomic>| {
        let start = Instant::now();
        let mut cursor = None;
        assert!(tree::claim_next(host, 64, &mut cursor, &mut |_| {}).is_none());
        start.elapsed()
    };
    let setup = |words: u32| {
        let host = TestHost::<Atomic>::new(WIDE.largest_plan(), Class::of_size(8).unwrap(), 1);
        let r0 = host.region_at(0);
        host.carve_all(r0, host.layout().slots());
        for w in 0..words {
            tree::release(&host, r0, w, u64::MAX);
        }
        host
    };
    for words in [512u32, 8192, 32768, 65536] {
        let (host, kept) = (setup(words), setup(words));
        let r0 = host.region_at(0);
        let mut claims = ExactRun::new();
        for w in 0..words {
            assert!(claims.claim(&host, r0, w, u64::MAX));
            let word = tree::word(&kept, tree::Node::leaf(kept.region_at(0), w));
            word.compare_exchange(u64::MAX, 0).unwrap();
        }
        let under_way = fill(&host);
        claims.finish(&host);
        let held = fill(&host);
        let left_set = fill(&kept);
        std::println!(
            "{words:6} words: claiming {under_way:>10.1?}, held {held:>10.1?}, bits left set {left_set:>10.1?}"
        );
    }
}
