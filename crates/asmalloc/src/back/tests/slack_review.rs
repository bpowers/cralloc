//! Evidence for the adversarial review of the trim at the claim and per-slot ageing
//! (`large-slack-rule` at `7e09b15`). Each test states what the branch says and fails where the
//! code does otherwise; the experiments print what they measured.

use super::super::hold::walk_aged;
use super::super::layout::{Recorded, STAMP_MASK};
use super::super::policy::{FINE_TICKS, HOLD_STEPS};
use super::super::region;
use super::super::testhost::TestHost;
use super::super::tree;
use super::super::word::{Atomic, BITS};
use super::*;
use crate::space::Geometry;
use crate::thread_word::{AWAKE, KEY_SHIFT, SLEEPING};
use core::sync::atomic::Ordering::Relaxed;

const KEY: u64 = 1 << KEY_SHIFT;

fn asleep<O: Os>(back: &Back<'_, O>) {
    back.state.thread.set(KEY | AWAKE | SLEEPING);
}

fn sleeping<O: Os>(back: &Back<'_, O>) -> bool {
    back.state.thread.load().sleeping()
}

/// The thread, woken: its purge steps until none would do more, then its plan. Returns how many
/// steps it took.
fn thread_runs<O: Os>(back: &Back<'_, O>) -> usize {
    let mut steps = 0;
    loop {
        steps += 1;
        if !back.thread_purge_steps(&|| back.now()).more {
            break;
        }
    }
    back.plan_hold();
    steps
}

/// `hold.rs`, "The thread": "a region is walked when it has something to return and no
/// oftener", and the branch's own test has the thread "woken about once a hold and not at every
/// step". That holds when the free slots were freed together. Freed one a fine step, as a
/// program whose demand falls slowly frees them, each matures at its own step, the walk's due
/// is the soonest of them, and the thread is woken at every fine step of the hold that follows:
/// a futex wake from a request's slow path and a full walk of the region each 2 ms, where the
/// region's own hold woke it once.
#[test]
#[cfg_attr(miri, ignore)]
fn slots_freed_a_step_apart_wake_the_thread_at_every_step() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    asleep(&back);
    back.state.hold.unfloored.store(true, Relaxed);
    let class = large(64 << 10);
    let size = class.size();
    let blocks: Vec<NonNull<u8>> = (0..16)
        .map(|_| back.alloc(class, size).unwrap().ptr)
        .collect();
    for b in &blocks {
        paint(b.as_ptr(), size, 7);
    }
    let mut wakes = 0;
    let mut step = |free: Option<NonNull<u8>>| {
        if let Some(b) = free {
            back.free(b, class);
        }
        // The churn that keeps the region young.
        back.free(blocks[0], class);
        assert_eq!(back.alloc(class, size).unwrap().ptr, blocks[0]);
        paint(blocks[0].as_ptr(), size, 7);
        os.advance_clock(FINE_TICKS);
        back.heartbeat();
        if !sleeping(&back) {
            wakes += 1;
            thread_runs(&back);
            asleep(&back);
        }
    };
    step(None);
    step(None);
    // From the top down, so that no later claim of the churn takes one.
    for b in blocks[1..].iter().rev() {
        step(Some(*b));
    }
    for _ in 0..2 * HOLD_STEPS {
        step(None);
    }
    for b in &blocks[1..] {
        // SAFETY: free slots, which nothing else uses meanwhile.
        assert!(unsafe { os.is_purged(*b, size) }, "an old slot stayed");
    }
    std::eprintln!(
        "woken {wakes} times for 15 slots over {} steps",
        17 + 2 * HOLD_STEPS
    );
    assert!(
        wakes <= 6,
        "woken {wakes} times in about two holds, for 15 slots freed a step apart"
    );
    back.free(blocks[0], class);
}

/// What the walks of one young region cost under steady churn, in the hold's own terms: a
/// region of 3000 slots of which about half are live, a few claimed (lowest first, as the
/// allocator claims) and a few freed at random each fine step, and every look a heartbeat's
/// (256 words). Before the branch a young region cost a look one load. The branch says a region
/// is walked "when it has something to return and no oftener".
#[test]
#[cfg_attr(miri, ignore)]
fn what_walking_a_young_region_costs_under_steady_churn() {
    let plan = Geometry::DESIGN.largest_plan();
    let class = large(40 << 10);
    let host = TestHost::<Atomic>::new(plan, class, 1);
    let region = host.region_at(0);
    let slots = 3000u32;
    host.carve_all(region, slots);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut live: Vec<u32> = (0..slots).collect();
    let stamp = |slot: u32, freed: u32| {
        let recorded = Recorded {
            size: class.size(),
            huge: false,
            unwritten: false,
            outgrown: false,
            freed,
        };
        region::record(&host, region, slot, recorded);
    };
    for slot in 0..slots {
        host.write(region, slot);
        stamp(slot, 0);
    }
    let steps = 4000u32;
    let (mut looks, mut words, mut walked_steps, mut returned) = (0usize, 0usize, 0usize, 0usize);
    let mut target = slots as usize / 2;
    for count in 1..=steps {
        // Demand drifts between a third and two thirds of the region.
        target = (target + rng.below(41))
            .saturating_sub(20)
            .clamp(1000, 2000);
        while live.len() > target {
            let slot = live.swap_remove(rng.below(live.len()));
            host.write(region, slot);
            stamp(slot, count & STAMP_MASK);
            tree::release_one(&host, region, slot / BITS, slot % BITS);
        }
        while live.len() < target {
            let taken = tree::claim(&host, 1, true).unwrap();
            let slot = taken.word * BITS + taken.bits.trailing_zeros();
            let last = region::recorded(&host, region, slot);
            let _ = region::settle_slot(&host, region, slot, class.size(), last);
            stamp(slot, 0);
            live.push(slot);
        }
        // Churn at the level: a few freed and taken again.
        for _ in 0..8 {
            let slot = live.swap_remove(rng.below(live.len()));
            host.write(region, slot);
            stamp(slot, count & STAMP_MASK);
            tree::release_one(&host, region, slot / BITS, slot % BITS);
            let taken = tree::claim(&host, 1, true).unwrap();
            let slot = taken.word * BITS + taken.bits.trailing_zeros();
            let last = region::recorded(&host, region, slot);
            let _ = region::settle_slot(&host, region, slot, class.size(), last);
            stamp(slot, 0);
            live.push(slot);
        }
        if count < 2 * HOLD_STEPS {
            continue;
        }
        let mut any = false;
        loop {
            let out = walk_aged(&host, region, count, HOLD_STEPS, 2 << 20, 256);
            if out.walked {
                looks += 1;
                words += out.read;
                any = true;
            }
            returned += out.returned;
            if out.done {
                break;
            }
        }
        walked_steps += usize::from(any);
    }
    let measured = (steps - 2 * HOLD_STEPS + 1) as usize;
    std::eprintln!(
        "{measured} fine steps: walked in {walked_steps}, {looks} looks, {} words a step, \
         {} KiB returned",
        words / measured,
        returned >> 10
    );
    assert!(
        walked_steps * 4 <= measured,
        "a young region was walked in {walked_steps} of {measured} fine steps, {} words a step",
        words / measured
    );
}

/// `trim_slack`: "A refusal leaves them resident and not clean, as they were". True, and so
/// every later claim of the slot finds the same slack and calls again. Where every purge is
/// refused (memory locked with `mlockall`, which the move's copy allows for:
/// `Dest::Unpurgeable`, "On locked memory every purge fails; stop trying"), each claim of a
/// free large slot that has slack makes one failing system call, for ever. Before the branch a
/// claim made none.
#[test]
#[cfg_attr(miri, ignore)]
fn where_purges_are_refused_every_claim_calls_and_fails() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = large(512 << 10);
    let size = class.size();
    let a = back.alloc(class, size).unwrap().ptr;
    paint(a.as_ptr(), size, 1);
    back.free(a, class);
    os.set_faults(
        Call::Purge,
        Faults {
            pass: 0,
            fail: u32::MAX,
            partial: false,
        },
    );
    let before = os.calls(Call::Purge);
    let rounds = 100;
    for _ in 0..rounds {
        let b = back.alloc(class, 40 << 10).unwrap().ptr;
        assert_eq!(b, a);
        back.free(b, class);
    }
    let calls = os.calls(Call::Purge) - before;
    os.set_faults(Call::Purge, Faults::NONE);
    assert!(
        calls <= 2,
        "{calls} refused purge calls in {rounds} claims of one slot"
    );
}

/// Liveness, at random: blocks of several large classes over several regions are taken and
/// freed at random with a heartbeat a fine step, then a few stay hot (freed and taken again
/// every step, which keeps their regions young) for four holds. Every other free slot has then
/// been free for more than a hold and must have gone back, with or without the thread.
fn idle_slots_go_back_beside_hot_ones(seed: u64, thread: bool) {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    back.state.hold.unfloored.store(true, Relaxed);
    if thread {
        asleep(&back);
    }
    let classes = [
        large(40 << 10),
        large(64 << 10),
        large(128 << 10),
        large(256 << 10),
    ];
    let mut rng = Rng(seed | 1);
    let mut live: Vec<(NonNull<u8>, Class)> = Vec::new();
    let beat = |back: &Back<'_, _>| {
        os.advance_clock(FINE_TICKS);
        back.heartbeat();
        if thread && !sleeping(back) {
            thread_runs(back);
            asleep(back);
        }
    };
    for _ in 0..600 {
        match rng.below(5) {
            0..=2 if live.len() < 120 => {
                let class = classes[rng.below(classes.len())];
                let size = 1 + rng.below(class.size());
                let ptr = back.alloc_staggered(class, size).unwrap().ptr;
                paint(ptr.as_ptr(), size, 9);
                live.push((ptr, class));
            }
            3 if !live.is_empty() => {
                let (ptr, class) = live.swap_remove(rng.below(live.len()));
                back.free_by_address(ptr);
                let _ = class;
            }
            _ => beat(&back),
        }
    }
    // A few stay hot; the rest go.
    while live.len() > 6 {
        let (ptr, _) = live.swap_remove(rng.below(live.len()));
        back.free_by_address(ptr);
        if rng.below(3) == 0 {
            beat(&back);
        }
    }
    for step in 0..4 * HOLD_STEPS {
        for held in live.iter_mut() {
            if rng.below(2) == 0 {
                back.free_by_address(held.0);
                let size = 1 + rng.below(held.1.size());
                held.0 = back.alloc_staggered(held.1, size).unwrap().ptr;
                paint(held.0.as_ptr(), size, 9);
            }
        }
        beat(&back);
        let _ = step;
    }
    // Hold the hot ones: what is left free has been free for a hold or more, but for the slots
    // the hot blocks left in their first steps here, also more than a hold ago.
    assert_eq!(
        back.retained_large(),
        0,
        "seed {seed:#x} thread {thread}: free large slots stayed resident past their hold"
    );
    for (ptr, _) in live {
        back.free_by_address(ptr);
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn idle_slots_go_back_beside_hot_ones_at_random() {
    for seed in 0..40u64 {
        for thread in [false, true] {
            idle_slots_go_back_beside_hot_ones(0x51ac_0000 + seed * 0x9e37_79b9, thread);
        }
    }
}

/// A gap the mutation pass found, closed: the walk's bound of words holds in a region whose
/// free slots are few, where it is the check between leaf words that stops a look. (Without
/// that check the branch's tests pass: their regions are dense with free slots.)
#[test]
#[cfg_attr(miri, ignore)]
fn a_look_at_a_sparse_region_stays_within_its_words() {
    let plan = Geometry::DESIGN.largest_plan();
    let class = large(40 << 10);
    let host = TestHost::<Atomic>::new(plan, class, 1);
    let region = host.region_at(0);
    let slots = 3000u32;
    host.carve_all(region, slots);
    let count = 5000u32;
    host.write(region, 0);
    let recorded = Recorded {
        size: class.size(),
        huge: false,
        unwritten: false,
        outgrown: false,
        freed: count - 3,
    };
    region::record(&host, region, 0, recorded);
    tree::release_one(&host, region, 0, 0);
    let mut looks = 0;
    loop {
        let out = walk_aged(&host, region, count, HOLD_STEPS, usize::MAX, 8);
        assert!(out.read <= 8 + 2, "a look read {} words", out.read);
        looks += 1;
        if out.done {
            break;
        }
        assert!(looks < 100, "the walk never reached the bottom");
    }
    assert!(
        looks >= 5,
        "{looks} looks over {} leaf words",
        slots / BITS + 1
    );
}

/// A gap the mutation pass found, closed: the thread's plan takes a walk's due. Fifteen slots
/// freed together and one ten steps later, in a region kept young: some plan made while the
/// last is young is for a step well under a hold on, where the region's own stamp gives a whole
/// hold. (With the walks left out of the plan the branch's tests pass.)
#[test]
#[cfg_attr(miri, ignore)]
fn the_threads_plan_is_for_the_step_a_walk_is_due() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    asleep(&back);
    back.state.hold.unfloored.store(true, Relaxed);
    let class = large(64 << 10);
    let size = class.size();
    let blocks: Vec<NonNull<u8>> = (0..16)
        .map(|_| back.alloc(class, size).unwrap().ptr)
        .collect();
    for b in &blocks {
        paint(b.as_ptr(), size, 7);
    }
    let mut planned = Vec::new();
    let mut step = |free: &[NonNull<u8>]| {
        for b in free {
            back.free(*b, class);
        }
        back.free(blocks[0], class);
        assert_eq!(back.alloc(class, size).unwrap().ptr, blocks[0]);
        paint(blocks[0].as_ptr(), size, 7);
        os.advance_clock(FINE_TICKS);
        back.heartbeat();
        if !sleeping(&back) {
            thread_runs(&back);
            asleep(&back);
            planned.push((back.hold_count(), back.hold_next()));
        }
    };
    step(&[]);
    step(&[]);
    step(&blocks[2..]);
    for _ in 0..9 {
        step(&[]);
    }
    step(&blocks[1..2]);
    for _ in 0..HOLD_STEPS + 20 {
        step(&[]);
    }
    // SAFETY: a free slot, which nothing else uses meanwhile.
    let gone = unsafe { os.is_purged(blocks[1], size) };
    assert!(gone, "the last slot stayed");
    let near = planned.iter().any(|&(count, next)| {
        let ahead = (next as u32).wrapping_sub(count);
        next & super::super::hold::NEXT_AT != 0 && ahead > 1 && ahead <= HOLD_STEPS / 4
    });
    assert!(near, "no plan took a walk's due: {planned:x?}");
    back.free(blocks[0], class);
}
