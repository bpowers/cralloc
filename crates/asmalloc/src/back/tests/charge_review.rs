//! Evidence for the adversarial review of the large-slot charge and the budget for kept grown
//! blocks (main `630883b..d6e8841`).
//!
//! The tests that are not ignored are the properties the review checked and found to hold: a
//! slot said to be zero reads as zero, and the retained count equals what the clean bits of
//! the free slots say, after every step of a random run and after a threaded one. The ignored
//! ones each fail on the code as reviewed; run one with
//! `cargo test -p asmalloc --lib back::tests::charge_review -- --ignored <name>`.

use super::super::layout::Grow;
use super::super::tree::{self, Host, Node};
use super::super::word::{BITS, Word};
use super::*;
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
}

/// Whether the page at `at`, a page boundary, holds a byte that is not zero.
fn holds_data(at: *const u8, page: usize) -> bool {
    // SAFETY: a page of a slot below a watermark, which is committed; the heap and these
    // tests store only integers there, and nothing else runs.
    let words = unsafe { core::slice::from_raw_parts(at.cast::<u64>(), page / 8) };
    words.iter().any(|&w| w != 0)
}

/// Reads every free large slot below a watermark. Nothing else may run.
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
        let carved = Grow::unpack(host.meta(region, layout.grow_word()).load()).carved;
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
        }
    }
    census
}

/// The large-slot retained count as it stands, signed.
fn count<S: Shape>(state: &State<Sim<S>>) -> isize {
    state.retained.large.load(Relaxed)
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
    /// Free slots still advised for huge pages: grown blocks kept with their advice.
    advised: usize,
    /// Free slots with some pages marked clean and some not: tails told apart.
    told: usize,
}

impl Met {
    fn add(&mut self, census: &Census) {
        self.hidden = self.hidden.max(census.hidden);
        self.zero += census.zero;
        self.advised += census.advised;
        self.told += census.told;
    }

    fn join(&mut self, other: Met) {
        self.hidden = self.hidden.max(other.hidden);
        self.zero += other.zero;
        self.advised += other.advised;
        self.told += other.told;
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
    let room = back.room(block.ptr.as_ptr()).unwrap();
    assert!(size <= room);
    if block.zeroed {
        assert!(
            painted(block.ptr.as_ptr(), room, 0),
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
            // SAFETY: both are live blocks the test holds, `from` given up, and `to` reads as
            // zero if the back end said so.
            unsafe { back.move_large(from.ptr, to.ptr, kept, zeroed) };
            assert!(
                bytes(to.ptr, kept) == from.shadow[..kept],
                "a move lost bytes"
            );
            if zeroed {
                // What the heap's zeroed grow relies on to skip clearing past what it kept.
                let room = back.room(to.ptr.as_ptr()).unwrap();
                assert!(
                    painted(to.ptr.as_ptr().wrapping_add(kept), room - kept, 0),
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
/// the clocked pass, the growth cap and trims between them. After every step: no free slot's
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
    for at in 0..steps {
        if faults && rng.below(6) == 0 {
            let call = [Call::Purge, Call::AdviseHuge][rng.below(2)];
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
    met
}

/// `seeds` runs; they must between them have met every kind of free slot the review is about.
fn runs(salt: u64, seeds: u64, steps: usize, faults: bool) {
    let mut met = Met::default();
    for seed in 0..seeds {
        met.join(run(salt ^ seed, steps, faults));
    }
    assert!(
        met.zero != 0 && met.advised != 0 && met.told != 0,
        "the runs missed a case: {met:?}"
    );
}

#[test]
#[cfg_attr(miri, ignore)]
fn the_count_follows_the_clean_bits_and_zero_slots_are_zero_quickly() {
    runs(0x6a09_e667_f3bc_c908, 2, 500, false);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_count_follows_the_clean_bits_and_zero_slots_are_zero_fully() {
    runs(0x6a09_e667_f3bc_c908, 24, 2500, false);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn the_count_follows_the_clean_bits_when_purges_and_advice_fail() {
    runs(0xbb67_ae85_84ca_a73b, 24, 2500, true);
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
            count(state),
            census.counted as isize,
            "round {round}: {census:?}"
        );
        back.trim();
        let census = self::census(&back);
        assert_eq!((census.resident, census.wrong_zero), (0, 0), "{census:?}");
        assert_eq!(count(state), 0, "round {round}");
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
/// when each is preempted during its copy.
#[test]
fn moves_under_way_at_once_each_keep_a_slot_past_the_budget() {
    const MOVERS: usize = 16;
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let budget = kept_grown(Native::GEOMETRY);
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
            back.move_large_with(from, to, len, false, || {
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
/// with that history, and neither the cap nor the budget for kept blocks sees any of it. Here
/// forty slots are written in full and freed, then each hosts a block a sixth of its size, as
/// a block that moved in with headroom is, and is freed again. The count is then under the cap
/// with five times the cap resident in free slots, so growth in another class pays nothing;
/// only the clocked pass returns it.
#[test]
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
