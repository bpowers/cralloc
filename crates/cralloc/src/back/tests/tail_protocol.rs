//! The tail word's protocol on a host that records the advice of each huge page
//! (`back::tail`): the take-over of a stale owner and its reconciliation, a taken-over owner's
//! give-back, what a give-back keeps of the marks, notes and repairs made during its
//! ownership, the reconciliation of a raise, and the mark's boundary. Each test fails on a wrong
//! version of its step that the other tests of the tail pass.

use super::super::layout::Layout;
use super::super::tail::{self, Change, Tail};
use super::super::testhost::TestHost;
use super::super::tree::Host;
use super::super::word::{Atomic, Word};
use crate::classes::Class;
use crate::os::HugePages;
use crate::os::sim::{Shape, Standard};
use crate::space::RegionId;
use std::boxed::Box;
use std::sync::Mutex;
use std::vec::Vec;

/// A host whose kernel keeps the advice of each huge page of region 0, counts the advice
/// calls, and runs a hook once, inside the first advice call it is given: what another thread
/// does while the caller is in the kernel.
struct Rec {
    host: TestHost<Atomic>,
    on: Mutex<Vec<bool>>,
    calls: Mutex<usize>,
    hook: Mutex<Option<Hook>>,
}

/// What another thread does while the caller is in the kernel.
type Hook = Box<dyn FnOnce(&Rec) + Send>;

impl Rec {
    fn new() -> Rec {
        let plan = Standard::GEOMETRY.largest_plan();
        let host = TestHost::<Atomic>::new(plan, Class::of_size(64).unwrap(), 1);
        let units = host.layout().region_pages() / host.layout().huge_pages();
        Rec {
            host,
            on: Mutex::new(std::vec![true; units as usize]),
            calls: Mutex::new(0),
            hook: Mutex::new(None),
        }
    }

    fn region(&self) -> RegionId {
        self.host.region_at(0)
    }

    /// The first page of the huge-page part: the region's second huge page.
    fn huge(&self) -> u32 {
        self.host.layout().huge_pages()
    }

    fn units(&self) -> u32 {
        self.host.layout().region_pages() / self.host.layout().huge_pages()
    }

    fn word(&self) -> Tail {
        Tail::unpack(self.host.tail(self.region()).load())
    }

    fn set(&self, tail: Tail) {
        self.host.tail(self.region()).store(tail.pack());
    }

    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }

    fn on_first_call(&self, hook: impl FnOnce(&Rec) + Send + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(hook));
    }

    /// The kernel's advice over the huge-page part is the word's.
    fn agrees(&self, tail: Tail) -> bool {
        let on = self.on.lock().unwrap();
        (1..self.units()).all(|unit| on[unit as usize] == (tail.from == 0 || unit < tail.from))
    }
}

impl Host for Rec {
    type W = Atomic;

    fn layout(&self) -> &Layout {
        self.host.layout()
    }
    fn geometry(&self) -> crate::space::Geometry {
        self.host.geometry()
    }
    fn region(&self, index: usize) -> Option<RegionId> {
        self.host.region(index)
    }
    fn meta(&self, region: RegionId, word: u32) -> &Atomic {
        self.host.meta(region, word)
    }
    fn tree_words(&self, region: RegionId, level: u8, count: u32) -> &[Atomic] {
        self.host.tree_words(region, level, count)
    }
    fn grow(&self, region: RegionId) -> &Atomic {
        self.host.grow(region)
    }
    fn hints(&self, region: RegionId) -> &Atomic {
        self.host.hints(region)
    }
    fn frontier(&self, region: RegionId) -> &Atomic {
        self.host.frontier(region)
    }
    fn tail(&self, region: RegionId) -> &Atomic {
        self.host.tail(region)
    }
    fn tail_marks(&self) -> &Atomic {
        self.host.tail_marks()
    }
    fn class_word(&self, index: u32) -> &Atomic {
        self.host.class_word(index)
    }
    fn class_top(&self) -> &Atomic {
        self.host.class_top()
    }
    fn commit(&self, region: RegionId, offset: usize, len: usize) -> bool {
        self.host.commit(region, offset, len)
    }
    fn commit_meta(&self, region: RegionId, offset: usize, len: usize) -> bool {
        self.host.commit_meta(region, offset, len)
    }
    fn advise(&self, _: RegionId, offset: usize, len: usize, advice: HugePages) -> bool {
        let unit = self.layout().huge_pages() as usize * self.layout().page_size();
        assert!(offset.is_multiple_of(unit) && len.is_multiple_of(unit) && len > 0);
        {
            let mut on = self.on.lock().unwrap();
            for at in offset / unit..(offset + len) / unit {
                on[at] = advice == HugePages::Prefer;
            }
        }
        *self.calls.lock().unwrap() += 1;
        let hook = self.hook.lock().unwrap().take();
        if let Some(hook) = hook {
            hook(self);
        }
        true
    }
    fn split(&self, region: RegionId, offset: usize, len: usize) -> bool {
        self.host.split(region, offset, len)
    }
    unsafe fn purge(&self, region: RegionId, offset: usize, len: usize) -> bool {
        // SAFETY: the caller's promise, passed on; no test here purges.
        unsafe { self.host.purge(region, offset, len) }
    }
    fn abort(&self, message: &str) -> ! {
        self.host.abort(message)
    }
}

/// An owner bit two intervals old is taken over, and the new owner reconciles first: here the
/// kernel's advice was left half changed (huge page 1 off below a boundary at 3), and after
/// the take-over and the lowering it agrees with the word. One interval old, or ahead of the
/// reader, the bit is the owner's and nothing is called.
#[test]
fn a_stale_owner_is_taken_over_and_the_advice_reconciled() {
    let rec = Rec::new();
    let region = rec.region();
    let owned = Tail {
        from: 3,
        low: Some(2),
        busy: true,
        stamp: 5,
        ..Tail::default()
    };
    rec.set(owned);
    rec.on.lock().unwrap()[1] = false;
    for epoch in [4, 5, 6] {
        assert_eq!(tail::lower(&rec, region, rec.huge(), epoch), Change::None);
        assert_eq!((rec.calls(), rec.word()), (0, owned), "interval {epoch}");
    }
    assert_eq!(tail::lower(&rec, region, rec.huge(), 7), Change::Done);
    let word = rec.word();
    assert_eq!(
        (word.from, word.low, word.busy, word.repair),
        (2, None, false, false)
    );
    assert!(rec.agrees(word), "{:?}", rec.on.lock().unwrap());
    // The reconcile (on below the boundary, off above) and the lowering.
    assert!(rec.calls() >= 3, "{} calls", rec.calls());
}

/// An owner taken over while it was in the kernel finds its bit gone when it gives it back,
/// and asks for a repair, since its call may have landed after the new owner's
/// reconciliation; a mark the word carries then sets the lookers' flag.
#[test]
fn a_taken_over_owner_asks_for_a_repair_when_it_gives_back() {
    let rec = Rec::new();
    let region = rec.region();
    tail::note(&rec, region, 2);
    rec.on_first_call(|rec| {
        // Intervals later, another thread takes the bit over, and a claim marks the region.
        let word = rec.host.tail(rec.region());
        let seen = Tail::unpack(word.load());
        let taken = Tail {
            stamp: seen.stamp + 9,
            want: true,
            ..seen
        };
        word.store(taken.pack());
    });
    assert_eq!(tail::lower(&rec, region, rec.huge(), 1), Change::Done);
    let word = rec.word();
    assert!(word.repair, "{word:?}");
    assert!(word.want);
    assert_ne!(rec.host.tail_marks().load(), 0, "the mark was left unseen");
}

/// What a claim, a walker or a taken-over owner writes into the word while an owner is in the
/// kernel survives the owner's give-back: the mark (and the lookers' flag), a note lower than
/// the owner's, and a repair request.
#[test]
fn a_give_back_keeps_what_was_written_meanwhile() {
    // The mark, against a lowering.
    let rec = Rec::new();
    let region = rec.region();
    tail::note(&rec, region, rec.units() - 1);
    let _ = tail::lower(&rec, region, rec.huge(), 1);
    tail::note(&rec, region, 2);
    rec.on_first_call(|rec| {
        assert!(tail::mark(rec, rec.region(), rec.layout().region_pages()));
    });
    assert_eq!(tail::lower(&rec, region, rec.huge(), 1), Change::Done);
    let word = rec.word();
    assert!(word.want && !word.busy, "{word:?}");
    assert_ne!(rec.host.tail_marks().load(), 0);

    // A lower note, against a lowering.
    let rec = Rec::new();
    let region = rec.region();
    tail::note(&rec, region, 3);
    rec.on_first_call(|rec| tail::note(rec, rec.region(), 2));
    assert_eq!(tail::lower(&rec, region, rec.huge(), 1), Change::Done);
    assert_eq!((rec.word().from, rec.word().low), (3, Some(2)));

    // A repair request, against a lowering.
    let rec = Rec::new();
    let region = rec.region();
    tail::note(&rec, region, 3);
    rec.on_first_call(|rec| {
        rec.host.tail(rec.region()).fetch_or(tail::REPAIR);
    });
    assert_eq!(tail::lower(&rec, region, rec.huge(), 1), Change::Done);
    assert!(rec.word().repair);
}

/// A raise that finds a repair asked for reconciles first: the advice the kernel was left
/// with (huge page 1 off) is put right before the advice goes back on.
#[test]
fn a_raise_reconciles_first() {
    let rec = Rec::new();
    let region = rec.region();
    tail::note(&rec, region, 3);
    let _ = tail::lower(&rec, region, rec.huge(), 1);
    rec.on.lock().unwrap()[1] = false;
    rec.host.tail(region).fetch_or(tail::REPAIR);
    assert!(tail::mark(&rec, region, rec.layout().region_pages()));
    let pages = rec.layout().region_pages();
    assert_eq!(
        tail::raise(&rec, region, rec.huge(), 0, pages, 1),
        Change::Done
    );
    let word = rec.word();
    assert!(!word.repair && !word.want, "{word:?}");
    assert!(rec.agrees(word), "{:?}", rec.on.lock().unwrap());
}

/// The reconciliation takes the advice off above the boundary as well as putting it on
/// below: a kernel that has every huge page advised for, under a word whose boundary is 2,
/// agrees with it after a repair.
#[test]
fn a_reconciliation_takes_the_advice_off_above_the_boundary() {
    let rec = Rec::new();
    let region = rec.region();
    rec.set(Tail {
        from: 2,
        repair: true,
        ..Tail::default()
    });
    assert_eq!(tail::lower(&rec, region, rec.huge(), 1), Change::None);
    let word = rec.word();
    assert!(!word.repair);
    assert!(rec.agrees(word), "{:?}", rec.on.lock().unwrap());
}

/// A claim marks the region only if it reached past the boundary's first page: a claim whose
/// slot ends on the boundary is below it.
#[test]
fn a_claim_marks_only_past_the_boundary() {
    let rec = Rec::new();
    let region = rec.region();
    rec.set(Tail {
        from: 2,
        ..Tail::default()
    });
    let at = 2 * rec.huge();
    assert!(!tail::mark(&rec, region, at));
    assert!(!rec.word().want);
    assert!(tail::mark(&rec, region, at + 1));
    assert!(rec.word().want);
    // Once marked, a second claim does not mark again.
    assert!(!tail::mark(&rec, region, at + 1));
}

/// The advice never comes off below the region's frontier as it stands when the lowering is
/// made: a huge page noted while it lay above the frontier, which fills have reached since,
/// keeps its advice, and the note is dropped. Once the frontier has aged, a note there lowers.
#[test]
fn a_lowering_stops_at_the_frontier() {
    let rec = Rec::new();
    let region = rec.region();
    let layout = *rec.layout();
    tail::note(&rec, region, 2);
    // A claim in interval 1 reaches into huge page 3.
    let last = layout.slot_at(3 * layout.huge_pages() as usize * layout.page_size());
    let _ = super::super::region::reach(&rec, region, last, 1);
    assert_eq!(tail::lower(&rec, region, rec.huge(), 1), Change::None);
    assert_eq!(rec.calls(), 0);
    assert_eq!(rec.word().low, None, "{:?}", rec.word());
    // Intervals later the claim no longer counts, and a note lowers.
    tail::note(&rec, region, 2);
    assert_eq!(tail::lower(&rec, region, rec.huge(), 9), Change::Done);
    assert_eq!(rec.word().from, 2);
}

/// A note at or above the boundary is no note: the advice there is off already.
#[test]
fn a_note_at_or_above_the_boundary_is_ignored() {
    let rec = Rec::new();
    let region = rec.region();
    rec.set(Tail {
        from: 2,
        ..Tail::default()
    });
    tail::note(&rec, region, 2);
    tail::note(&rec, region, 3);
    assert_eq!(rec.word().low, None);
    tail::note(&rec, region, 1);
    assert_eq!(rec.word().low, Some(1));
}

/// The advice never comes off below the start of the huge-page part: a note there (a huge
/// page of the small-page part, which a walk never returns in part) lowers nothing.
#[test]
fn a_lowering_stays_within_the_huge_page_part() {
    let rec = Rec::new();
    let region = rec.region();
    tail::note(&rec, region, 1);
    // The huge-page part starts at huge page 2.
    assert_eq!(tail::lower(&rec, region, 2 * rec.huge(), 1), Change::None);
    assert_eq!(rec.calls(), 0);
    assert_eq!(rec.word().from, 0);
}

/// A fork child's repair (`Back::distrust_tails`) may clear the bit of a live owner of the
/// child's own: a second thread the child started before its first slow path, in the middle of
/// a lowering. That owner is taken over: its give-back asks for a repair, and the next change
/// makes the advice agree with the word.
#[test]
fn a_child_repair_that_clears_a_live_owner_takes_it_over() {
    let rec = Rec::new();
    let region = rec.region();
    tail::note(&rec, region, 2);
    rec.on_first_call(|rec| {
        // The repair, as `distrust_tails` makes it, while the owner is in the kernel.
        let word = rec.host.tail(rec.region());
        let seen = word.load();
        word.compare_exchange(seen, (seen & !tail::BUSY) | tail::REPAIR)
            .unwrap();
    });
    let _ = tail::lower(&rec, region, rec.huge(), 1);
    let word = rec.word();
    assert!(word.repair && !word.busy, "{word:?}");
    rec.on.lock().unwrap()[1] = false;
    // The owner's word was never written: its note is still there, and the next lowering
    // reconciles and makes it.
    assert_eq!(tail::lower(&rec, region, rec.huge(), 1), Change::Done);
    let word = rec.word();
    assert_eq!((word.from, word.repair), (2, false), "{word:?}");
    assert!(rec.agrees(word), "{:?}", rec.on.lock().unwrap());
}
