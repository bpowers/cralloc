//! Evidence for the adversarial review of the large-slot charge and the budget for kept grown
//! blocks (main `630883b..d6e8841`), and for the check of its fix (main `b313200`). The budget
//! has since become the count of kept outgrown slots (`Back::keep_outgrown`), which the tests
//! here check too: the random runs and the threaded ones hold it to the charge of the slots
//! marked kept at every step.
//!
//! The properties: a slot said to be zero reads as zero; a clean bit is true, so none stands
//! over a page that holds data or under what a held block may write; and the retained count
//! is the bytes of the free slots' pages that are not clean. They are checked after every step
//! of random runs against the back end and against the heap, the second by a program that
//! fills all it is told it may use, and after threaded runs. Three tests are the first
//! review's findings, which failed on the code as reviewed and pass since the fix.
//!
//! The slow runs do less in a debug build, which is what `scripts/gate --all` runs, than in a
//! release one: `cargo test --release -p asmalloc --lib charge_review -- --include-ignored`
//! runs them in full in about a minute.

use super::super::layout::Grow;
use super::super::tree::{self, Host, Node};
use super::super::word::{BITS, Word};
use super::*;
use crate::SimHeap;
use crate::classes::{C_ALIGN, MAX_SMALL_SIZE};
use crate::heap::Found;
use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::Ordering::Relaxed;

/// What the bitmaps and the simulated memory say of the free large slots.
#[derive(Default, Debug)]
struct Census {
    /// Bytes of their pages that are not marked clean: what the retained count should be.
    counted: usize,
    /// Bytes of their pages that hold data.
    resident: usize,
    /// Bytes of their pages that hold data and are marked clean: resident, and in no count.
    hidden: usize,
    /// The same for the slots the program holds: a clean bit over a page the tenant wrote.
    hidden_held: usize,
    /// Slots whose zero bit is set though some byte of them is not zero.
    wrong_zero: usize,
    /// Slots whose zero bit is set.
    zero: usize,
    /// Slots whose size word still says they are advised for huge pages.
    advised: usize,
    /// Slots with some pages marked clean and some not.
    told: usize,
    /// The charge of the free slots marked kept by the move their block outgrew them in:
    /// what the count of kept outgrown slots should be.
    outgrown: usize,
    /// How many free slots are so marked.
    kept: usize,
}

/// Whether the `len` bytes at `at` all equal `byte`. A comparison of whole pages, which is a
/// library call and as fast in a debug build as in a release one: a census reads every page
/// of every large slot.
fn all_are(at: *const u8, len: usize, byte: u8) -> bool {
    let like = [byte; 4096];
    // SAFETY: the tests read only slots below a watermark, which are committed; the heap and
    // these tests store only integers there.
    let bytes = unsafe { core::slice::from_raw_parts(at, len) };
    bytes
        .chunks(like.len())
        .all(|chunk| chunk == &like[..chunk.len()])
}

/// Whether the page at `at` holds a byte that is not zero.
fn holds_data(at: *const u8, page: usize) -> bool {
    !all_are(at, page, 0)
}

/// The pages marked clean under the `len` bytes at `ptr`, a block in a large slot. The block
/// may write all of them, so there should be none. Nothing for a run or a small slot.
fn clean_under<S: Shape>(back: &Back<'_, Sim<S>>, ptr: NonNull<u8>, len: usize) -> usize {
    let Some(Kind::Slot(class)) = back.lookup(ptr.as_ptr()) else {
        return 0;
    };
    if class.is_small() || len == 0 {
        return 0;
    }
    let host = back.host(class).unwrap();
    let layout = *host.layout();
    let location = back.space().locate(ptr.as_ptr()).unwrap();
    let offset = location.offset as usize;
    let first = (offset >> layout.page_shift()) as u32;
    let end = (offset + len).div_ceil(layout.page_size()) as u32;
    (first..end)
        .filter(|&p| {
            let word = host.meta(location.region, layout.clean_word(p / BITS));
            word.load() >> (p % BITS) & 1 != 0
        })
        .count()
}

/// Reads every large slot below a watermark. Nothing else may run.
fn census<S: Shape>(back: &Back<'_, Sim<S>>) -> Census {
    let mut census = Census::default();
    for region in back.space().plan().regions() {
        let RegionTag::Class(class) = back.table().get(region) else {
            continue;
        };
        if class.is_small() {
            continue;
        }
        let host = back.host(class).unwrap();
        let layout = *host.layout();
        let page = layout.page_size();
        let carved = Grow::unpack(host.grow(region).load()).carved;
        for slot in 0..carved.min(layout.slots()) {
            let (word, bit) = (slot / BITS, slot % BITS);
            if tree::word(&host, Node::leaf(region, word)).load() >> bit & 1 == 0 {
                // Held: no clean bit may stand over a page that holds data.
                let unit = layout.slot_unit(slot).unwrap();
                let start = back.space().slot(region, class, slot).as_ptr();
                for p in unit.start..unit.end {
                    let clean =
                        host.meta(region, layout.clean_word(p / BITS)).load() >> (p % BITS) & 1;
                    let at = start.wrapping_add((p - unit.start) as usize * page);
                    if clean != 0 && holds_data(at, page) {
                        census.hidden_held += page;
                    }
                }
                continue;
            }
            let zero = host.meta(region, layout.zero_word(word).unwrap()).load() >> bit & 1 != 0;
            let unit = layout.slot_unit(slot).unwrap();
            let start = back.space().slot(region, class, slot).as_ptr();
            let (mut dirty, mut clean_pages) = (false, 0);
            for p in unit.start..unit.end {
                let clean = host.meta(region, layout.clean_word(p / BITS)).load() >> (p % BITS) & 1;
                let at = start.wrapping_add((p - unit.start) as usize * page);
                let data = holds_data(at, page);
                if clean == 0 {
                    census.counted += page;
                } else {
                    clean_pages += 1;
                }
                if data {
                    census.resident += page;
                    dirty = true;
                    if clean != 0 {
                        census.hidden += page;
                    }
                }
            }
            census.zero += usize::from(zero);
            census.wrong_zero += usize::from(zero && dirty);
            census.told += usize::from(clean_pages != 0 && clean_pages != unit.end - unit.start);
            let advised = super::super::region::recorded(&host, region, slot);
            census.advised += usize::from(advised.is_some_and(|r| r.huge));
            if advised.is_some_and(|r| r.outgrown) {
                census.outgrown += super::super::region::charge(&host, region, slot);
                census.kept += 1;
            }
        }
    }
    census
}

/// The large-slot retained count as it stands, signed.
fn count<S: Shape>(state: &State<Sim<S>>) -> isize {
    state.retained.signed(true)
}

fn bytes(ptr: NonNull<u8>, len: usize) -> Vec<u8> {
    // SAFETY: the tests read only blocks the back end handed out, within their slots.
    unsafe { core::slice::from_raw_parts(ptr.as_ptr(), len) }.to_vec()
}

/// What a run met, summed over its steps except where noted, so that a pass is known to have
/// exercised the paths under review.
#[derive(Default, Debug, Clone, Copy)]
struct Met {
    /// The most bytes resident in free slots and marked clean at one step.
    hidden: usize,
    /// Free slots with their zero bit set.
    zero: usize,
    /// Free slots still advised for huge pages, which only a refused call to take the advice
    /// back leaves.
    advised: usize,
    /// Free slots with some pages marked clean and some not: tails told apart.
    told: usize,
    /// Free slots kept when their blocks outgrew them.
    kept: usize,
    /// Bytes the hold returned from regions frees kept young, by the slots' own ages.
    aged: usize,
}

impl Met {
    fn add(&mut self, census: &Census) {
        self.hidden = self.hidden.max(census.hidden);
        self.zero += census.zero;
        self.advised += census.advised;
        self.told += census.told;
        self.kept += census.kept;
    }

    fn join(&mut self, other: Met) {
        self.hidden = self.hidden.max(other.hidden);
        self.zero += other.zero;
        self.advised += other.advised;
        self.told += other.told;
        self.kept += other.kept;
        self.aged += other.aged;
    }
}

/// A live block and what the program wrote in it.
struct Held {
    ptr: NonNull<u8>,
    class: Class,
    shadow: Vec<u8>,
}

/// Allocates a block of `class` holding `size` bytes and checks what the back end says of it.
fn take<O: Os>(back: &Back<'_, O>, rng: &mut Rng, class: Class, size: usize) -> (Held, bool) {
    let block = if rng.below(2) == 0 {
        back.alloc_staggered(class, size)
    } else {
        back.alloc(class, size)
    }
    .unwrap();
    // Two blocks in three are marked as a zeroed request from a slot known to be zero would
    // be, whatever the slot's history: the free's sample and its return must keep every
    // property below whatever the mark says (`Back::mark_unwritten`).
    if rng.below(3) != 0 {
        back.mark_unwritten(block.ptr);
    }
    let room = back.room(block.ptr.as_ptr()).unwrap();
    assert!(size <= room);
    if block.zeroed {
        assert!(
            all_are(block.ptr.as_ptr(), room, 0),
            "a slot of {class:?} was said to be zero and is not"
        );
    }
    let held = Held {
        ptr: block.ptr,
        class,
        shadow: bytes(block.ptr, size),
    };
    (held, block.zeroed)
}

/// A size for a block of `class`: all of it, any part, a small part or a page or two.
fn some_size(rng: &mut Rng, class: Class, page: usize) -> usize {
    match rng.below(5) {
        0 => class.size(),
        1 => 1 + rng.below(class.size()),
        2 | 3 => 1 + rng.below(class.size() / 4),
        _ => 1 + rng.below(2 * page),
    }
}

/// Writes part of a block, or all of it, as a program would, and records it.
fn scribble(rng: &mut Rng, held: &mut Held, byte: u8) {
    let len = held.shadow.len();
    let (from, to) = match rng.below(3) {
        0 => (0, len),
        _ => {
            let from = rng.below(len);
            (from, from + 1 + rng.below(len - from))
        }
    };
    paint(held.ptr.as_ptr().wrapping_add(from), to - from, byte);
    held.shadow[from..to].fill(byte);
}

fn intact(held: &Held) -> bool {
    bytes(held.ptr, held.shadow.len()) == held.shadow
}

/// One random step on `live`, which holds at most `most` blocks: allocate, write, free, resize
/// in place, record a size, or move.
fn step<O: Os>(
    back: &Back<'_, O>,
    rng: &mut Rng,
    classes: &[Class],
    live: &mut Vec<Held>,
    byte: u8,
    most: usize,
) {
    let page = back.space().plan().geometry().page_size();
    let pick = |rng: &mut Rng, live: &Vec<Held>| rng.below(live.len());
    match rng.below(12) {
        0..=2 if live.len() < most => {
            let class = classes[rng.below(classes.len())];
            let size = some_size(rng, class, page);
            let (mut held, _) = take(back, rng, class, size);
            if rng.below(4) != 0 {
                scribble(rng, &mut held, byte);
            }
            live.push(held);
        }
        3 | 4 if !live.is_empty() => {
            let at = pick(rng, live);
            scribble(rng, &mut live[at], byte);
            if rng.below(3) == 0 {
                // And the rest of what it may use, as a C program that asks for its usable size
                // does. A grow reads what is there, so the shadow needs none of it.
                let held = &live[at];
                let usable = back.usable_size(held.ptr.as_ptr()).unwrap();
                let size = held.shadow.len();
                paint(held.ptr.as_ptr().wrapping_add(size), usable - size, byte);
            }
        }
        5 | 6 if !live.is_empty() => {
            let held = live.swap_remove(pick(rng, live));
            assert!(intact(&held), "a live block changed");
            if rng.below(2) == 0 {
                back.free(held.ptr, held.class);
            } else {
                back.free_by_address(held.ptr);
            }
        }
        7 | 8 if !live.is_empty() => {
            let at = pick(rng, live);
            let held = &mut live[at];
            let room = back.room(held.ptr.as_ptr()).unwrap();
            let size = match rng.below(3) {
                0 => 1 + rng.below(room),
                1 => (held.shadow.len() * 2).min(room),
                _ => 1 + rng.below(held.shadow.len()),
            };
            let old = held.shadow.len();
            if rng.below(4) == 0 {
                back.resize(held.ptr, size);
            } else {
                // SAFETY: the test holds the block and uses none of it during the call.
                unsafe { back.resize_in_place(held.ptr, size) };
            }
            assert_eq!(back.size(held.ptr.as_ptr()), Some(size));
            held.shadow.truncate(size);
            assert!(intact(held), "a resize in place changed what it kept");
            if size > old {
                // What a grow exposes is indeterminate; the program reads what is there.
                held.shadow = bytes(held.ptr, size);
            }
        }
        9 | 10 if !live.is_empty() => {
            // As the heap moves a block: a new slot, its size recorded, then the move.
            let from = live.swap_remove(pick(rng, live));
            assert!(intact(&from), "a live block changed");
            let class = classes[rng.below(classes.len())];
            let size = some_size(rng, class, page);
            let asked = if rng.below(2) == 0 {
                class.size()
            } else {
                size
            };
            let (mut to, zeroed) = take(back, rng, class, asked);
            back.resize(to.ptr, size);
            let kept = from.shadow.len().min(size);
            // Half the moves are not outgrowths, so that the free inside such a move meets every
            // history the runs make. `REVIEW_OTHER=0` makes every move an outgrowth, as the
            // review's mutation runs had them.
            let why = if std::env::var_os("REVIEW_OTHER").is_some_and(|v| v == "0")
                || rng.below(2) == 0
            {
                crate::back::Move::Outgrown
            } else {
                crate::back::Move::Other
            };
            // SAFETY: both are live blocks the test holds, `from` given up, and `to` reads as
            // zero if the back end said so.
            unsafe { back.move_large(from.ptr, to.ptr, kept, zeroed, why) };
            assert!(
                bytes(to.ptr, kept) == from.shadow[..kept],
                "a move lost bytes"
            );
            if zeroed {
                // What the heap's zeroed grow relies on to skip clearing past what it kept.
                let room = back.room(to.ptr.as_ptr()).unwrap();
                assert!(
                    all_are(to.ptr.as_ptr().wrapping_add(kept), room - kept, 0),
                    "a move into a zero slot left it not zero past what it copied"
                );
            }
            to.shadow = bytes(to.ptr, size);
            live.push(to);
        }
        _ => {}
    }
}

/// The large classes of the native shape the runs use: under a huge page (charged whole),
/// one huge page, and several above, so that blocks move between slots that tell their tails
/// apart and slots that do not.
fn classes() -> Vec<Class> {
    [
        40 << 10,
        64 << 10,
        96 << 10,
        128 << 10,
        192 << 10,
        256 << 10,
        512 << 10,
    ]
    .into_iter()
    .map(large)
    .collect()
}

/// Random allocations, writes, frees, resizes and moves of large blocks on one thread, with
/// the clocked pass, the growth cap, the hold (with no floor, so that it acts on a heap this
/// small) and trims between them. After every step: no free slot's
/// zero bit is set over a byte that is not zero, and the retained count is the bytes of the
/// free slots' pages that are not marked clean. After a trim, when nothing fails, no free slot
/// holds data. Returns what the run met.
fn run(seed: u64, steps: usize, faults: bool) -> Met {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let mut rng = Rng(seed | 1);
    let classes = classes();
    let mut live: Vec<Held> = Vec::new();
    let (mut byte, mut met) = (0u8, Met::default());
    state
        .hold
        .unfloored
        .store(true, core::sync::atomic::Ordering::Relaxed);
    for at in 0..steps {
        if faults && rng.below(6) == 0 {
            let call = [Call::Purge, Call::AdviseHuge, Call::Resident][rng.below(3)];
            let plan = match rng.below(3) {
                0 => Faults::NONE,
                1 => Faults::once_after(rng.below(3) as u32),
                _ => Faults {
                    pass: rng.below(3) as u32,
                    fail: 1 + rng.below(3) as u32,
                    partial: rng.below(2) == 0,
                },
            };
            os.set_faults(call, plan);
        }
        byte = byte.wrapping_add(1).max(1);
        let mut trimmed = false;
        match rng.below(16) {
            0 => {
                os.advance_clock(DECAY_TICKS);
                for _ in 0..1 + rng.below(8) {
                    back.heartbeat();
                }
            }
            1 if rng.below(3) == 0 => {
                back.trim();
                trimmed = true;
            }
            // Fine steps of the hold's count, a beat each, so that free slots age by it within
            // a few dozen steps of the run, as they do within a fraction of an interval in a
            // program.
            2..=4 => {
                for _ in 0..1 + rng.below(16) {
                    os.advance_clock(super::super::policy::FINE_TICKS);
                    back.heartbeat();
                }
            }
            _ => step(&back, &mut rng, &classes, &mut live, byte, 24),
        }
        let census = census(&back);
        assert_eq!(census.wrong_zero, 0, "seed {seed:#x} step {at}");
        // Clean bits are true: no page marked clean holds data, free or held.
        assert_eq!(
            (census.hidden, census.hidden_held),
            (0, 0),
            "seed {seed:#x} step {at}: a clean bit over a written page ({census:?})"
        );
        assert_eq!(
            count(&state),
            census.counted as isize,
            "seed {seed:#x} step {at}: the count against the clean bits ({census:?})"
        );
        assert_eq!(
            back.outgrown(),
            census.outgrown,
            "seed {seed:#x} step {at}: the count of kept outgrown slots ({census:?})"
        );
        // Nor does one stand under what a held block may write, written yet or not.
        for held in &live {
            let usable = back.usable_size(held.ptr.as_ptr()).unwrap();
            assert_eq!(
                clean_under(&back, held.ptr, usable),
                0,
                "seed {seed:#x} step {at}: a clean bit under a block of {usable} bytes"
            );
        }
        if trimmed && !faults {
            assert_eq!(
                census.resident, 0,
                "seed {seed:#x} step {at}: a trim left data in free slots ({census:?})"
            );
        }
        met.add(&census);
    }
    for call in Call::ALL {
        os.set_faults(call, Faults::NONE);
    }
    for held in live.drain(..) {
        assert!(intact(&held), "a live block changed");
        back.free(held.ptr, held.class);
    }
    back.trim();
    let census = census(&back);
    assert_eq!((census.resident, census.wrong_zero), (0, 0), "{census:?}");
    assert_eq!(count(&state), 0, "seed {seed:#x}: left after the last trim");
    assert_eq!(
        back.outgrown(),
        0,
        "seed {seed:#x}: kept outgrown slots after the last trim"
    );
    met.aged = state
        .hold
        .aged_returned
        .load(core::sync::atomic::Ordering::Relaxed);
    met
}

/// How much less a slow run does in a debug build, where a step costs several times what it
/// does in a release one: `scripts/gate --all` runs debug builds under a time limit, and each
/// of these then takes ten seconds or less.
const DEBUG_LESS: u64 = if cfg!(debug_assertions) { 4 } else { 1 };

/// `seeds` runs; they must between them have met every kind of free slot the review is about.
fn runs(salt: u64, seeds: u64, steps: usize, faults: bool) {
    let mut met = Met::default();
    for seed in 0..seeds {
        met.join(run(salt ^ seed, steps, faults));
    }
    assert!(
        met.zero != 0 && met.told != 0 && met.kept != 0 && met.aged != 0,
        "the runs missed a case: {met:?}"
    );
    // No free slot keeps a grown block's advice: its free and the move that keeps the slot it
    // outgrew both take it back, which only a refused call leaves in place.
    if !faults {
        assert_eq!(met.advised, 0, "advice stayed on a free slot: {met:?}");
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn the_count_follows_the_clean_bits_and_zero_slots_are_zero_quickly() {
    runs(0x6a09_e667_f3bc_c908, 3, 600, false);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_count_follows_the_clean_bits_and_zero_slots_are_zero_fully() {
    runs(0x6a09_e667_f3bc_c908, 24 / DEBUG_LESS, 2500, false);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_count_follows_the_clean_bits_when_purges_and_advice_fail() {
    runs(0xbb67_ae85_84ca_a73b, 24 / DEBUG_LESS, 2500, true);
}

/// The same steps on several threads at once, each with its own blocks, while another trims
/// and beats the heart: every slot said to be zero is zero, every block keeps its bytes, and
/// once the threads stop the count is what the clean bits say, and nothing after a trim.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn racing_tenants_and_purges_leave_the_count_at_the_clean_bits() {
    for round in 0..6u64 {
        let state = state::<Native>();
        state.get_or_reserve().unwrap();
        let state = &*state;
        let classes = classes();
        let classes = &classes[..];
        let done = core::sync::atomic::AtomicBool::new(false);
        thread::scope(|scope| {
            let workers: Vec<_> = (0..12u64)
                .map(|t| {
                    scope.spawn(move || {
                        let back = state.get().unwrap();
                        let mut rng = Rng(0x3c6e_f372_fe94_f82b ^ (round << 8) ^ t | 1);
                        let mut live = Vec::new();
                        for at in 0..4000usize {
                            let byte = (at as u8).max(1);
                            step(&back, &mut rng, classes, &mut live, byte, 6);
                        }
                        for held in live {
                            assert!(intact(&held), "a live block changed");
                            back.free(held.ptr, held.class);
                        }
                    })
                })
                .collect();
            scope.spawn(|| {
                let back = state.get().unwrap();
                let mut at = 0u32;
                while !done.load(Relaxed) {
                    at += 1;
                    if at.is_multiple_of(16) {
                        back.trim();
                    }
                    state.os().advance_clock(DECAY_TICKS / 4);
                    back.heartbeat();
                    thread::yield_now();
                }
            });
            for worker in workers {
                worker.join().unwrap();
            }
            done.store(true, Relaxed);
        });
        let back = state.get().unwrap();
        let census = census(&back);
        assert_eq!(census.wrong_zero, 0, "round {round}");
        assert_eq!(
            (census.hidden, census.hidden_held),
            (0, 0),
            "round {round}: a clean bit over a written page ({census:?})"
        );
        assert_eq!(
            count(state),
            census.counted as isize,
            "round {round}: {census:?}"
        );
        // Every change to the count of kept outgrown slots raced with every other, and it is
        // still the charge of the slots marked kept: nothing drifts.
        assert_eq!(
            back.outgrown(),
            census.outgrown,
            "round {round}: {census:?}"
        );
        back.trim();
        let census = self::census(&back);
        assert_eq!((census.resident, census.wrong_zero), (0, 0), "{census:?}");
        assert_eq!(count(state), 0, "round {round}");
        assert_eq!(back.outgrown(), 0, "round {round}");
    }
}

/// Finding empty-extent. A tenant whose recorded size is zero, in a slot with no stagger, has
/// an empty extent, so its free marks every page of the slot clean and charges nothing. If an
/// earlier tenant left the slot resident, the slot is then free, resident, in no count, and
/// wholly clean, which is what the purge walk skips (`all_clean`): neither the clocked pass
/// nor a trim ever returns it. `charge_resident`'s argument that the walk still reaches a
/// clean, resident tail ("the extent's pages are not clean, so the unit is not skipped")
/// assumes an extent of at least one page.
///
/// The heap reaches this with a zero-size request aligned to a huge page or more
/// (`Class::of_aligned` gives it a slot; C's `aligned_alloc`, `memalign` and `posix_memalign`
/// accept the size), and
/// `a_zero_size_aligned_block_leaves_its_slot_resident_for_good` in `heap::address` shows it.
#[test]
#[cfg_attr(miri, ignore)]
fn a_tenant_that_asked_for_nothing_hides_a_resident_slot_from_the_purge() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(Native::GEOMETRY.huge_page_size());
    let first = back.alloc(class, class.size()).unwrap().ptr;
    paint(first.as_ptr(), class.size(), 0x77);
    back.free(first, class);
    assert_eq!(back.retained(), class.size());
    let empty = back.alloc(class, 0).unwrap();
    assert_eq!(empty.ptr, first);
    assert!(!empty.zeroed);
    back.free(empty.ptr, class);
    let census = census(&back);
    assert_eq!(census.resident, class.size(), "the slot is still resident");
    // The count and the bits agree with each other, and with what is resident. (As reviewed,
    // both said nothing was.)
    assert_eq!(
        (back.retained(), census.counted),
        (class.size(), class.size())
    );
    os.advance_clock(DECAY_TICKS);
    for _ in 0..64 {
        back.heartbeat();
    }
    back.trim();
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    let purged = unsafe { os.is_purged(first, class.size()) };
    assert!(
        purged,
        "a free slot holds {} resident bytes after a decay pass and a trim",
        self::census(&back).resident
    );
}

/// Finding budget-per-mover. `move_large_with` decides to keep the slot it moves out of
/// before the copy and counts the slot after it, so every move that is under way at once
/// sees the count as it was and keeps its slot: the kept memory goes past the budget by one
/// slot for each mover, not by one slot, and each of those may be as large as the budget.
/// (`purge_before_release` has the same gap, a few instructions wide.) Here the movers are
/// one thread whose moves nest at the first piece, which is the order sixteen threads take
/// when each is preempted during its copy. Fixed: a move counts its slot before it decides
/// (`keep_outgrown`), so the kept slots pass the bound by one slot at most.
#[test]
#[cfg_attr(miri, ignore)]
fn moves_under_way_at_once_each_keep_a_slot_past_the_budget() {
    const MOVERS: usize = 16;
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let budget = kept_outgrown(Native::GEOMETRY);
    let (class, wider) = (large(192 << 10), large(256 << 10));
    let len = class.size() - 100;
    let blocks: Vec<(NonNull<u8>, NonNull<u8>)> = (0..MOVERS)
        .map(|_| {
            let from = back.alloc(class, len).unwrap().ptr;
            pattern(from.as_ptr(), len);
            (from, back.alloc(wider, wider.size()).unwrap().ptr)
        })
        .collect();
    fn nest<O: Os>(back: &Back<'_, O>, blocks: &[(NonNull<u8>, NonNull<u8>)], len: usize) {
        let Some((&(from, to), rest)) = blocks.split_first() else {
            return;
        };
        let mut first = true;
        // SAFETY: both are live blocks the test holds, `from` given up.
        unsafe {
            back.move_large_with(from, to, len, false, crate::back::Move::Outgrown, || {
                if core::mem::take(&mut first) {
                    nest(back, rest, len);
                }
            });
        }
        assert!(patterned(to.as_ptr(), len));
    }
    nest(&back, &blocks, len);
    let kept = back.retained_large();
    assert!(
        kept <= budget + class.size(),
        "{MOVERS} moves at once kept {kept} bytes of the slots they left; the budget is \
         {budget} and a slot is {}",
        class.size()
    );
}

/// Finding hidden-from-the-cap. The under-count the author states (a tenant after a larger one
/// leaves the larger one's pages resident and marked clean) is bounded for one slot, by the
/// slot less the tenant's extent, but not in total: it is one such tail for every free slot
/// with that history, and neither the cap nor the count of kept outgrown slots sees any of it. Here
/// forty slots are written in full and freed, then each hosts a block a sixth of its size, as
/// a block that moved in with headroom is, and is freed again. The count is then under the cap
/// with five times the cap resident in free slots, so growth in another class pays nothing;
/// only the clocked pass returns it.
#[test]
#[cfg_attr(miri, ignore)]
fn tenants_after_larger_ones_hide_free_memory_from_the_cap() {
    const SLOTS: usize = 40;
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let cap = retained_cap(Native::GEOMETRY);
    let class = large(256 << 10);
    let fill = |size: usize| -> Vec<NonNull<u8>> {
        (0..SLOTS)
            .map(|_| {
                let ptr = back.alloc(class, size).unwrap().ptr;
                paint(ptr.as_ptr(), size, 0x6b);
                ptr
            })
            .collect()
    };
    for ptr in fill(class.size()) {
        back.free(ptr, class);
    }
    assert_eq!(back.retained(), SLOTS * class.size());
    for ptr in fill(40 << 10) {
        back.free(ptr, class);
    }
    let before = census(&back);
    assert_eq!(before.resident, SLOTS * class.size());
    // The count is what is resident. (As reviewed, it was under the cap.)
    assert_eq!(back.retained(), before.resident);
    // A phase change: another class grows by as much as is free and resident.
    let other = large(128 << 10);
    let grown: Vec<_> = (0..before.resident / other.size())
        .map(|_| back.alloc(other, other.size()).unwrap().ptr)
        .collect();
    let after = census(&back);
    assert!(
        after.resident <= cap + class.size(),
        "{} bytes are free and resident after the heap grew by {}; the cap is {cap} and the \
         count says {}",
        after.resident,
        grown.len() * other.size(),
        back.retained()
    );
}

/// A block a program holds through the heap.
struct Owned {
    ptr: NonNull<u8>,
    /// A layout that fits it, for a block of the Rust calls; none for one of the C calls.
    layout: Option<Layout>,
    /// The bytes the program wrote, from the block's start: all it was told it may use.
    wrote: usize,
    fill: u8,
}

/// C's `realloc` of a large block to a large size, as `c::realloc` does it on the process's
/// heap: in place if the block's room allows, else a new block (with headroom if it grows)
/// and a move of everything the old one's usable size covered.
fn c_realloc(sim: &SimHeap<Native>, old: NonNull<u8>, size: usize) -> NonNull<u8> {
    let heap = sim.heap();
    let Some(Found::Large { size: used }) = heap.find(old.as_ptr()) else {
        panic!("not a large block");
    };
    // SAFETY: the test holds the block and uses none of it during the call.
    if unsafe { heap.resize_in_place_by_address(old, used, size) } {
        return old;
    }
    let kept = heap.usable_size(old.as_ptr()).unwrap();
    let new = if size > kept {
        heap.alloc_large_grown(size)
    } else {
        heap.alloc_large_aligned(size, C_ALIGN, false)
    }
    .unwrap();
    // SAFETY: both are live blocks the test holds, `old` given up.
    unsafe {
        heap.back().unwrap().move_large(
            old,
            new,
            kept.min(size),
            false,
            crate::back::Move::Outgrown,
        )
    };
    new
}

/// A size a large request may have: mostly one that takes a slot, of any class the native
/// shape's regions hold, sometimes one that takes a run.
fn large_size(rng: &mut Rng) -> usize {
    match rng.below(12) {
        0 => (1 << 20) + 1 + rng.below(2 << 20),
        1..=4 => MAX_SMALL_SIZE + 1 + rng.below(64 << 10),
        _ => MAX_SMALL_SIZE + 1 + rng.below(700 << 10),
    }
}

/// A program on the heap that fills all it is told it may use: what `malloc_usable_size`
/// reports of a block of the C calls, the layout's size of a block of the Rust ones, and the
/// length a zeroed grow returns. Blocks are allocated plain, zeroed, aligned (some with a
/// size of zero or a few bytes), with headroom by a growing `realloc`, resized in place and
/// moved, and freed, with collections and the clocked pass between. After every step the
/// clean bits are true (none over a page that holds data, none under a held block), no zero
/// bit stands over data, the count is the free slots' pages that are not clean, and memory
/// promised zero is zero. A heap that claimed a slot for less than it then let the program
/// write fails the check after that step.
fn fill_all(seed: u64, steps: usize) -> Met {
    let sim = SimHeap::<Native>::new();
    let os = sim.os();
    let heap = sim.heap();
    let page = Native::GEOMETRY.page_size();
    let mut rng = Rng(seed | 1);
    let mut live: Vec<Owned> = Vec::new();
    let (mut fill, mut met) = (0u8, Met::default());
    let write = |ptr: NonNull<u8>, len: usize, fill: u8| paint(ptr.as_ptr(), len, fill);
    for at in 0..steps {
        fill = fill.wrapping_add(1).max(1);
        let pick = |rng: &mut Rng, live: &Vec<Owned>, c: bool| {
            let of: Vec<usize> = (0..live.len())
                .filter(|&i| live[i].layout.is_none() == c)
                .collect();
            (!of.is_empty()).then(|| of[rng.below(of.len())])
        };
        match rng.below(16) {
            // C: malloc, calloc, aligned_alloc.
            0..=3 if live.len() < 20 => {
                let (size, align, zero) = match rng.below(4) {
                    0 => (large_size(&mut rng), C_ALIGN, true),
                    1 => {
                        let size = [0, 1, 100, 5000, 40 << 10, 200 << 10][rng.below(6)];
                        (size, (64 << 10) << rng.below(3), rng.below(4) == 0)
                    }
                    _ => (large_size(&mut rng), C_ALIGN, false),
                };
                let ptr = heap.alloc_large_aligned(size, align, zero).unwrap();
                assert_eq!(ptr.addr().get() % align, 0);
                let usable = heap.usable_size(ptr.as_ptr()).unwrap();
                assert!(usable >= size);
                if zero {
                    assert!(
                        all_are(ptr.as_ptr(), usable, 0),
                        "seed {seed:#x} step {at}: a zeroed block of {size} bytes is not zero"
                    );
                }
                write(ptr, usable, fill);
                live.push(Owned {
                    ptr,
                    layout: None,
                    wrote: usable,
                    fill,
                });
            }
            // C: realloc.
            4..=6 => {
                if let Some(i) = pick(&mut rng, &live, true) {
                    let old = &live[i];
                    let size = large_size(&mut rng).min(900 << 10);
                    let ptr = c_realloc(&sim, old.ptr, size);
                    let usable = heap.usable_size(ptr.as_ptr()).unwrap();
                    assert!(usable >= size);
                    assert!(
                        all_are(ptr.as_ptr(), old.wrote.min(size), old.fill),
                        "seed {seed:#x} step {at}: realloc to {size} lost bytes"
                    );
                    write(ptr, usable, fill);
                    live[i] = Owned {
                        ptr,
                        layout: None,
                        wrote: usable,
                        fill,
                    };
                }
            }
            // C: free.
            7 => {
                if let Some(i) = pick(&mut rng, &live, true) {
                    let old = live.swap_remove(i);
                    assert!(all_are(old.ptr.as_ptr(), old.wrote, old.fill));
                    // SAFETY: the test's block, freed once.
                    unsafe { heap.free_large(old.ptr) };
                }
            }
            // Rust: alloc and alloc_zeroed, small blocks among them.
            8 | 9 if live.len() < 20 => {
                let size = match rng.below(4) {
                    0 => 1 + rng.below(MAX_SMALL_SIZE),
                    _ => large_size(&mut rng),
                };
                let align = [8, 16, page, 64 << 10][rng.below(4)];
                let layout = Layout::from_size_align(size, align).unwrap();
                let zero = rng.below(3) == 0;
                // SAFETY: the layout's size is not zero.
                let ptr = unsafe {
                    if zero {
                        GlobalAlloc::alloc_zeroed(&sim, layout)
                    } else {
                        GlobalAlloc::alloc(&sim, layout)
                    }
                };
                let ptr = NonNull::new(ptr).unwrap();
                if zero {
                    assert!(
                        all_are(ptr.as_ptr(), size, 0),
                        "seed {seed:#x} step {at}: alloc_zeroed of {size} bytes is not zero"
                    );
                }
                write(ptr, size, fill);
                live.push(Owned {
                    ptr,
                    layout: Some(layout),
                    wrote: size,
                    fill,
                });
            }
            // Rust: realloc, and a zeroed grow.
            10..=12 => {
                if let Some(i) = pick(&mut rng, &live, false) {
                    let old = &live[i];
                    let layout = old.layout.unwrap();
                    let grow_zeroed = rng.below(3) == 0;
                    let size = match rng.below(4) {
                        _ if grow_zeroed => layout.size() + 1 + rng.below(300 << 10),
                        0 => 1 + rng.below(MAX_SMALL_SIZE),
                        1 => (layout.size() * 2).min(900 << 10),
                        _ => large_size(&mut rng),
                    };
                    let new = Layout::from_size_align(size, layout.align()).unwrap();
                    let kept = old.wrote.min(layout.size()).min(size);
                    let (ptr, wrote) = if grow_zeroed {
                        // SAFETY: the test's block, which `layout` fits; `new` is larger.
                        let (ptr, len) =
                            unsafe { sim.grow_zeroed_for_test(old.ptr.as_ptr(), layout, new) }
                                .unwrap();
                        assert!(len >= size);
                        assert_eq!(
                            sim.usable_size(ptr),
                            Some(len),
                            "seed {seed:#x} step {at}: a grant of {len} bytes for {size}"
                        );
                        // From the old layout's size to the length returned, whether the
                        // block stayed or moved.
                        let from = layout.size().min(len);
                        assert!(
                            all_are(ptr.wrapping_add(from), len - from, 0),
                            "seed {seed:#x} step {at}: a zeroed grow to {size} is not zero"
                        );
                        (ptr, len)
                    } else {
                        // SAFETY: the test's block, which `layout` fits; `size` is not zero.
                        (
                            unsafe { GlobalAlloc::realloc(&sim, old.ptr.as_ptr(), layout, size) },
                            size,
                        )
                    };
                    let ptr = NonNull::new(ptr).unwrap();
                    assert!(
                        all_are(ptr.as_ptr(), kept, old.fill),
                        "seed {seed:#x} step {at}: a resize to {size} lost bytes"
                    );
                    write(ptr, wrote, fill);
                    // Any size from the one asked for to the length granted fits the block.
                    let fits = Layout::from_size_align(wrote, layout.align()).unwrap();
                    live[i] = Owned {
                        ptr,
                        layout: Some(fits),
                        wrote,
                        fill,
                    };
                }
            }
            // Rust: dealloc.
            13 => {
                if let Some(i) = pick(&mut rng, &live, false) {
                    let old = live.swap_remove(i);
                    assert!(all_are(old.ptr.as_ptr(), old.wrote, old.fill));
                    // SAFETY: the test's block, which the layout fits, freed once.
                    unsafe { GlobalAlloc::dealloc(&sim, old.ptr.as_ptr(), old.layout.unwrap()) };
                }
            }
            14 => {
                os.advance_clock(DECAY_TICKS);
                sim.maintain();
            }
            _ if rng.below(4) == 0 => {
                sim.collect();
            }
            _ => {}
        }
        let Some(back) = heap.back() else {
            continue;
        };
        let census = census(&back);
        assert_eq!(census.wrong_zero, 0, "seed {seed:#x} step {at}");
        assert_eq!(
            (census.hidden, census.hidden_held),
            (0, 0),
            "seed {seed:#x} step {at}: a clean bit over a written page ({census:?})"
        );
        for owned in &live {
            let usable = heap.usable_size(owned.ptr.as_ptr()).unwrap();
            assert!(owned.wrote <= usable);
            assert_eq!(
                clean_under(&back, owned.ptr, usable),
                0,
                "seed {seed:#x} step {at}: a clean bit under a block of {usable} bytes"
            );
        }
        assert_eq!(
            back.state.retained.signed(true),
            census.counted as isize,
            "seed {seed:#x} step {at}: the count against the clean bits ({census:?})"
        );
        assert_eq!(
            back.outgrown(),
            census.outgrown,
            "seed {seed:#x} step {at}: the count of kept outgrown slots ({census:?})"
        );
        met.add(&census);
    }
    for owned in live.drain(..) {
        assert!(all_are(owned.ptr.as_ptr(), owned.wrote, owned.fill));
        // SAFETY: the test's blocks, each freed once.
        unsafe {
            match owned.layout {
                Some(layout) => GlobalAlloc::dealloc(&sim, owned.ptr.as_ptr(), layout),
                None => heap.free_large(owned.ptr),
            }
        }
    }
    sim.collect();
    let back = heap.back().unwrap();
    let census = census(&back);
    assert_eq!((census.resident, census.wrong_zero), (0, 0), "{census:?}");
    assert_eq!(back.state.retained.signed(true), 0);
    assert_eq!(back.outgrown(), 0, "kept outgrown slots after the collect");
    met
}

fn fills(seeds: u64, steps: usize) {
    let mut met = Met::default();
    for seed in 0..seeds {
        met.join(fill_all(0x510e_527f_ade6_82d1 ^ seed, steps));
    }
    assert!(
        met.zero != 0 && met.told != 0 && met.kept != 0,
        "the runs missed a case: {met:?}"
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn a_program_that_fills_all_it_may_use_meets_no_clean_page_quickly() {
    fills(2, 300);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_program_that_fills_all_it_may_use_meets_no_clean_page_fully() {
    fills(16 / DEBUG_LESS, 2500);
}

/// Moves on many threads at once, each out of a block of its own into a block it keeps, so
/// that the only slots freed are those the moves leave and every one is a decision to keep or
/// return. A decision counts its slot before it looks (`keep_outgrown`), and here no claim can
/// take a slot out of the count before it is in, so the slots kept never pass the bound, at
/// any moment and so at the end; no update of the count is lost; and the count of kept
/// outgrown slots is the charge of the free slots, and zero once a trim has returned them.
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn racing_moves_keep_within_the_budget() {
    let budget = kept_outgrown(Native::GEOMETRY);
    // The block moved into is more than twice the one moved, so it is never advised for huge
    // pages, and it is freed only after the count has been read.
    let (class, wider) = (large(96 << 10), large(256 << 10));
    let len = class.size() - 100;
    for round in 0..8 {
        let state = state::<Native>();
        state.get_or_reserve().unwrap();
        let state = &*state;
        let most = core::sync::atomic::AtomicUsize::new(0);
        let into: Vec<usize> = (0..16)
            .map(|_| {
                let back = state.get().unwrap();
                back.alloc(wider, wider.size()).unwrap().ptr.addr().get()
            })
            .collect();
        thread::scope(|scope| {
            for &to in &into {
                let most = &most;
                scope.spawn(move || {
                    let back = state.get().unwrap();
                    let to = back.space().pointer(to).unwrap();
                    for at in 0..600usize {
                        let stamp = (at as u8).max(1);
                        let from = back.alloc(class, len).unwrap().ptr;
                        paint(from.as_ptr(), len, stamp);
                        // SAFETY: both are live blocks this thread holds, `from` given up.
                        unsafe {
                            back.move_large(from, to, len, false, crate::back::Move::Outgrown)
                        };
                        assert!(all_are(to.as_ptr(), len, stamp));
                        most.fetch_max(back.retained_large(), Relaxed);
                    }
                });
            }
        });
        let back = state.get().unwrap();
        let census = census(&back);
        assert_eq!(count(state), census.counted as isize, "round {round}");
        // Every free slot is one a move left.
        let kept = census.counted;
        assert!(
            kept <= budget,
            "round {round}: {kept} bytes of the slots moved out of are kept; the budget is {budget}"
        );
        // A reading during the run also holds the charges of decisions not yet taken back.
        let most = most.load(Relaxed);
        assert!(
            most <= budget + 16 * class.size(),
            "round {round}: the count reached {most}"
        );
        assert_ne!(kept, 0, "round {round}: nothing was kept at all");
        assert_eq!(
            back.outgrown(),
            kept,
            "round {round}: the count is the kept slots' charge"
        );
        for &to in &into {
            back.free(back.space().pointer(to).unwrap(), wider);
        }
        back.trim();
        assert_eq!(count(state), 0, "round {round}");
        assert_eq!(
            back.outgrown(),
            0,
            "round {round}: the trim took every kept slot"
        );
    }
}

mod headroom_review;

/// More seeds of the random runs, which make half their moves not outgrowths, with and without
/// failing calls (from the review of `outgrown-count`).
#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn more_seeds_with_moves_of_both_kinds() {
    runs(0x243f_6a88_85a3_08d3, 40 / DEBUG_LESS, 2500, false);
    runs(0x1319_8a2e_0370_7344, 60 / DEBUG_LESS, 2500, true);
}
