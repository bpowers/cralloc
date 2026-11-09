//! What the walks by slot ages cost and how soon they return a slot (`hold::walk_aged`; `hold`,
//! "The thread"): how often the thread is woken, the words a walk of a young region reads, and
//! how long a free slot waits. And what a claim of a slot with slack costs where purges are
//! refused (`Back::trim_slack`). The experiments print what they measured.

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

/// A region is walked when it has something to return and no oftener, and the thread is woken
/// about once a hold and not at every step, also when the free slots were freed apart. Freed one a
/// fine step, as a program whose demand falls slowly frees them, each matures at its own step; a
/// walk due at the soonest of them would wake the thread at every fine step of the hold that
/// follows, a futex wake from a request's slow path and a full walk of the region each 2 ms.
#[test]
#[cfg_attr(miri, ignore)]
fn slots_freed_a_step_apart_do_not_wake_the_thread_at_every_step() {
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
/// (256 words). A region is walked when it has something to return and no oftener.
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
            let _ = region::settle_slot(&host, region, slot, class.size(), 0, last);
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
            let _ = region::settle_slot(&host, region, slot, class.size(), 0, last);
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

/// `trim_slack`: "A refusal leaves them resident and not clean, as they were", so every later
/// claim of the slot finds the same slack and calls again. Where every purge is refused (memory
/// locked with `mlockall`), each claim of a free large slot that has slack makes one failing
/// system call: the documented limit (`hold`, "Where purges are refused"), and no more than
/// one a claim.
#[test]
#[cfg_attr(miri, ignore)]
fn where_purges_are_refused_a_claim_makes_one_failing_call() {
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
    // The first claim follows a tenant that wrote the whole slot, and has no slack.
    assert_eq!(
        calls,
        rounds - 1,
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

/// The walk's bound of words holds in a region whose free slots are few, where it is the check
/// between leaf words that stops a look. (The other tests of the walks have regions dense with
/// free slots, which do not need that check.)
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

/// How long a free slot waits before it goes back, a young region being walked at most once a
/// hold, in a region that a free keeps young every `every` fine steps, with and without the
/// thread. The slots are freed five steps apart. Returns the longest wait in fine steps and the
/// thread's wakes.
fn longest_wait(every: u32, thread: bool) -> (u32, u32) {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    back.state.hold.unfloored.store(true, Relaxed);
    if thread {
        asleep(&back);
    }
    let class = large(64 << 10);
    let size = class.size();
    let blocks: Vec<NonNull<u8>> = (0..16)
        .map(|_| back.alloc(class, size).unwrap().ptr)
        .collect();
    for b in &blocks {
        paint(b.as_ptr(), size, 7);
    }
    let mut freed: Vec<(NonNull<u8>, u32)> = Vec::new();
    let mut waiting = blocks[1..].to_vec();
    let (mut longest, mut wakes) = (0, 0);
    for step in 0..8 * HOLD_STEPS {
        if step % 5 == 0
            && let Some(b) = waiting.pop()
        {
            back.free(b, class);
            freed.push((b, step));
        }
        if step % every == 0 {
            back.free(blocks[0], class);
            assert_eq!(back.alloc(class, size).unwrap().ptr, blocks[0]);
            paint(blocks[0].as_ptr(), size, 7);
        }
        os.advance_clock(FINE_TICKS);
        back.heartbeat();
        if thread && !sleeping(&back) {
            wakes += 1;
            thread_runs(&back);
            asleep(&back);
        }
        freed.retain(|&(b, at)| {
            // SAFETY: a free slot, which nothing else uses meanwhile.
            let gone = unsafe { os.is_purged(b, size) };
            if gone {
                longest = longest.max(step + 1 - at);
            }
            !gone
        });
    }
    assert!(
        freed.is_empty() && waiting.is_empty(),
        "every {every} thread {thread}: {} slots never went back",
        freed.len()
    );
    back.free(blocks[0], class);
    (longest, wakes)
}

/// Every slot goes back within a bounded number of holds, however often a free keeps its
/// region young, and the thread is woken about once a hold.
#[test]
#[cfg_attr(miri, ignore)]
fn a_slot_waits_a_bounded_number_of_holds_whatever_keeps_its_region_young() {
    for every in [1, 2, 3, 7, 20, 50, 63] {
        for thread in [false, true] {
            let (longest, wakes) = longest_wait(every, thread);
            std::eprintln!(
                "a free every {every}, thread {thread}: longest wait {longest}, wakes {wakes}"
            );
            assert!(
                longest <= 3 * HOLD_STEPS + 8,
                "a free every {every}, thread {thread}: a slot waited {longest} steps"
            );
            assert!(
                wakes <= 8 * 2 + 2,
                "every {every}: woken {wakes} times in 8 holds"
            );
        }
    }
}
