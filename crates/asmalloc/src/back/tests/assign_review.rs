//! Evidence for the check of the fix to the growth-headroom review's last-region finding (main
//! `9a076f4`): `Back::assign` commits a large class's first slot before it takes a region,
//! writes the commit into the region's grow word once it holds the tag, and clears that word
//! (`unprepare`) when it loses the race for the class's carving word and gives the region up.
//!
//! Assignment is in no TLA+ model and no exploration: `models::carving` explores
//! `region::carve` on a region that is already assigned, and the table, the carving word and
//! the header a region gets on the way are real atomics the explorer cannot schedule. So this
//! is threads racing on a simulated heap, many times over, and a look at every region once
//! they stop.
//!
//! What must hold then, whatever the order:
//!
//! - every block is a slot of its class's region, handed out once, and writable;
//! - a region's grow word counts exactly the slots handed out of it, and the commit it records
//!   happened;
//! - a region that carries a tag and has handed nothing out is its class's carving region, so
//!   no region is taken and left;
//! - a large class's region has its first slot committed;
//! - an unassigned region is all zero: both header words, where a large class keeps them and
//!   where a small class does;
//! - a small class's region, of which nothing was freed, has an empty summary: the words a
//!   large class's header would have been.

use super::super::layout::{Grow, WORD_BYTES};
use super::super::tree::Host;
use super::super::word::Word;
use super::*;
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering::SeqCst;
use std::sync::Barrier;

/// What one thread of a round allocates: these classes in turn, a slot of each.
type Wants = Vec<Class>;

/// A block a thread allocated, on its way to the check.
struct Held(Class, NonNull<u8>);

// SAFETY: the thread that allocated the block hands it over and does not use it again.
unsafe impl Send for Held {}

/// The threads of a round. Four take half regions of one class, so that every other
/// allocation assigns a region and they race for the class's carving word: the loser gives its
/// region up and carves from the winner's, so the region it gave up waits for whoever assigns
/// next. Two take whole regions, where the loser takes its own region straight back. Three
/// take a slot of a small class each time, a different class every time, so that small classes
/// take regions the large ones gave up.
fn threads() -> Vec<Wants> {
    let region = Native::GEOMETRY.largest_plan().region_size();
    let smalls: Vec<Class> = SmallClass::all().map(SmallClass::class).collect();
    let mut all = Vec::new();
    for _ in 0..4 {
        all.push(std::vec![large(region / 2); 12]);
    }
    for _ in 0..2 {
        all.push(std::vec![large(region); 6]);
    }
    for t in 0..3 {
        all.push((0..12).map(|i| smalls[3 * i + t]).collect());
    }
    all
}

/// The first two words of a region's metadata block, if the block is committed there: a large
/// class's grow and hints words, and the top of a small class's summary.
fn block_head<S: Shape>(
    back: &Back<'_, Sim<S>>,
    region: crate::space::RegionId,
) -> Option<[u64; 2]> {
    let block = back.space().metadata(region);
    if !back.state.os().is_writable(block, 2 * WORD_BYTES) {
        return None;
    }
    let words = block.cast::<AtomicU64>();
    // SAFETY: the two words are committed, aligned, and only ever accessed atomically.
    Some(unsafe {
        [
            words.as_ref().load(SeqCst),
            words.add(1).as_ref().load(SeqCst),
        ]
    })
}

/// Checks every region of a heap at rest against the blocks `held`, as the module
/// documentation lists. Returns the number of regions that carry a class's tag.
fn check(state: &State<Sim<Native>>, held: &[Held], round: u64) -> usize {
    let back = state.get().unwrap();
    let os = state.os();
    let plan = back.space().plan();
    let page = plan.geometry().page_size();
    let mut handed = BTreeMap::<usize, BTreeSet<u32>>::new();
    for &Held(class, ptr) in held {
        let location = back.space().locate(ptr.as_ptr()).unwrap();
        assert_eq!(
            back.table().get(location.region),
            RegionTag::Class(class),
            "round {round}: a block of {class:?} in another class's region"
        );
        let layout = *back.host(class).unwrap().layout();
        let (slot, into) = layout.block_at(location.offset as usize).unwrap();
        assert_eq!(into, 0);
        assert!(
            os.is_writable(ptr, class.size()),
            "round {round}: a block of {class:?} that is not writable"
        );
        assert!(
            handed
                .entry(location.region.index())
                .or_default()
                .insert(slot),
            "round {round}: a slot of {class:?} handed out twice"
        );
    }
    let mut tagged = 0;
    for region in plan.regions() {
        let at = region.index();
        let small_header = &back.state.headers[at];
        match back.table().get(region) {
            RegionTag::Unassigned => {
                assert!(!handed.contains_key(&at));
                assert_eq!(
                    (small_header.grow.load(), small_header.hints.load()),
                    (0, 0),
                    "round {round}: unassigned region {at} has a small class's header"
                );
                if let Some(head) = block_head(&back, region) {
                    assert_eq!(
                        head,
                        [0, 0],
                        "round {round}: unassigned region {at} has a large class's header"
                    );
                }
            }
            RegionTag::Class(class) => {
                tagged += 1;
                let host = back.host(class).unwrap();
                let layout = *host.layout();
                let grow = Grow::unpack(host.grow(region).load());
                let slots = handed.get(&at).map_or(0, BTreeSet::len);
                assert_eq!(
                    grow.carved as usize, slots,
                    "round {round}: region {at} of {class:?} carved {grow:?}"
                );
                let committed = grow.committed as usize * page;
                assert!(layout.slot_bytes(grow.carved) <= committed);
                // Nothing below the class's origin is ever committed.
                let origin = layout.slot_bytes(0);
                assert!(
                    committed == 0
                        || os.is_writable(back.space().slot(region, class, 0), committed - origin),
                    "round {round}: region {at} of {class:?} records a commit that did not \
                     happen ({grow:?})"
                );
                let carving = host.class.carving.load(SeqCst) == at as u32 + 1;
                assert!(
                    grow.carved != 0 || carving,
                    "round {round}: region {at} was taken for {class:?} and left ({grow:?})"
                );
                if class.is_small() {
                    // Nothing was freed, so the summary is empty. Its top words are where a
                    // large class that had the region first kept its header.
                    assert_eq!(
                        block_head(&back, region),
                        Some([0, 0]),
                        "round {round}: region {at} of {class:?} has a summary before any free"
                    );
                } else {
                    assert!(
                        committed >= class.size(),
                        "round {round}: region {at} of {class:?} was taken before its first \
                         slot was committed ({grow:?})"
                    );
                }
            }
            other => panic!("round {round}: region {at} is {other:?}"),
        }
    }
    tagged
}

/// One round: the threads allocate at once on a fresh heap, with commits failing if `faults`;
/// then the heap is checked. Returns the times a region was taken and given up, and the
/// allocations refused.
///
/// A thread that takes a region advises it once (`Back::prepare`), whether it then keeps the
/// region or loses the race for the carving word and gives it up, and nothing else here
/// advises. So the advice calls less the regions that ended with a tag are the regions given
/// up.
fn round(seed: u64, faults: bool) -> (usize, usize) {
    let state = state::<Native>();
    state.get_or_reserve().unwrap();
    let state = &*state;
    let advised = state.os().calls(Call::AdviseHuge);
    let mut rng = Rng(seed | 1);
    if faults {
        state.os().set_faults(
            Call::Commit,
            Faults {
                pass: rng.below(60) as u32,
                fail: 1 + rng.below(6) as u32,
                partial: rng.below(2) == 0,
            },
        );
    }
    let wants = threads();
    let barrier = Barrier::new(wants.len());
    let (held, refused) = thread::scope(|scope| {
        let workers: Vec<_> = wants
            .iter()
            .map(|classes| {
                let barrier = &barrier;
                scope.spawn(move || {
                    let back = state.get().unwrap();
                    let (mut got, mut refused) = (Vec::new(), 0);
                    barrier.wait();
                    for &class in classes {
                        match back.alloc(class, class.size()) {
                            Some(block) => got.push(Held(class, block.ptr)),
                            None => refused += 1,
                        }
                    }
                    (got, refused)
                })
            })
            .collect();
        let mut held = Vec::new();
        let mut refused = 0;
        for worker in workers {
            let (got, none) = worker.join().unwrap();
            held.extend(got);
            refused += none;
        }
        (held, refused)
    });
    state.os().set_faults(Call::Commit, Faults::NONE);
    assert!(faults || refused == 0, "round {seed:#x}: {refused} refused");
    let tagged = check(state, &held, seed);
    let taken = (state.os().calls(Call::AdviseHuge) - advised) as usize;
    assert!(
        taken >= tagged,
        "round {seed:#x}: {tagged} tagged, {taken} taken"
    );
    (taken - tagged, refused)
}

fn rounds(salt: u64, count: u64, faults: bool) {
    let (mut given_up, mut refused) = (0, 0);
    for at in 0..count {
        let (regions, none) = round(salt ^ at, faults);
        given_up += regions;
        refused += none;
    }
    // Some race for a carving word must have been lost, or the rounds did not reach
    // `unprepare`. On one CPU the threads may never meet.
    let cpus = thread::available_parallelism().map_or(1, usize::from);
    assert!(
        given_up != 0 || cpus < 2,
        "no region was given up in {count} rounds"
    );
    assert!(!faults || refused != 0, "no commit was refused");
}

#[test]
#[cfg_attr(miri, ignore)]
fn racing_assignments_leave_every_region_whole_quickly() {
    rounds(0x243f_6a88_85a3_08d3, 60, false);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn racing_assignments_leave_every_region_whole_fully() {
    rounds(0x243f_6a88_85a3_08d3, 1500, false);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn racing_assignments_with_commits_refused_leave_every_region_whole() {
    rounds(0x1319_8a2e_0370_7344, 1500, true);
}
