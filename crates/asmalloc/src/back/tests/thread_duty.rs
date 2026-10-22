//! Returning memory while the background thread runs (`purge`, "While the background thread
//! runs"; `hold`, "The thread"): heartbeats return nothing and wake the thread when work they
//! did not do comes due, the thread's steps return what heartbeats would have, at the same
//! moments, and a grower pays a step itself once the thread has left too much unpaid.
//!
//! The back end does not know generations, so a word naming any key in state `AWAKE` reads as
//! a running thread here. The thread's part is played by [`thread_runs`], called when a
//! heartbeat or a grower woke it.

use super::super::policy::{
    DECAY_TICKS, FINE_TICKS, HOLD_STEPS, allowance_floor, growth_step, growth_unpaid, retained_cap,
};
use super::*;
use crate::thread_word::{AWAKE, KEY_SHIFT, SLEEPING};

/// Any generation's key: the back end reads only the state.
const KEY: u64 = 1 << KEY_SHIFT;

/// The thread marks its word asleep, as it does before a timed wait.
fn asleep<O: Os>(back: &Back<'_, O>) {
    back.state.thread.set(KEY | AWAKE | SLEEPING);
}

/// Whether the word still says the thread sleeps: a waker clears the mark.
fn sleeping<O: Os>(back: &Back<'_, O>) -> bool {
    back.state.thread.load().sleeping()
}

/// The thread, woken: the purge steps until none would do more, and its plan for the hold.
/// Returns what it returned.
fn thread_runs<O: Os>(back: &Back<'_, O>) -> usize {
    let before = back.retained();
    loop {
        let step = back.purge_steps(&|| back.now());
        if !step.more {
            break;
        }
    }
    back.plan_hold();
    before.saturating_sub(back.retained())
}

/// A heartbeat, and the thread's run if it woke it, after which the thread sleeps again.
/// Returns whether it was woken.
fn beat_and_serve<O: Os>(back: &Back<'_, O>) -> bool {
    back.heartbeat();
    if sleeping(back) {
        return false;
    }
    thread_runs(back);
    asleep(back);
    true
}

/// `bytes` of slots of the large class `class` freed after two fine steps, with one more slot
/// live above them; returns the live one.
fn free_large<O: Os>(back: &Back<'_, O>, os: &SimOs<Native>, class: Class, bytes: usize) -> Block {
    let size = class.size();
    let blocks: Vec<Block> = (0..bytes / size)
        .map(|_| back.alloc(class, size).unwrap())
        .collect();
    let top = back.alloc(class, size).unwrap();
    for b in &blocks {
        paint(b.ptr.as_ptr(), size, 7);
    }
    for _ in 0..2 {
        os.advance_clock(FINE_TICKS);
        back.heartbeat();
    }
    for b in &blocks {
        back.free(b.ptr, class);
    }
    top
}

/// The hold's end, with the thread running: the heartbeats of the hold return nothing and wake
/// nobody, the one whose reading steps the fine count to the hold's end wakes the thread, at
/// the same step a heartbeat would have taken the look itself, and no heartbeat after it
/// returns anything or wakes it again; the thread returns what is over the floor.
#[test]
#[cfg_attr(miri, ignore)]
fn the_step_that_ends_a_hold_wakes_the_thread_and_returns_nothing_itself() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    asleep(&back);
    let class = large(256 << 10);
    let floor = allowance_floor(Native::GEOMETRY);
    let top = free_large(&back, os, class, 2 * floor);
    back.plan_hold();
    let (freed, purges) = (back.retained_large(), os.calls(Call::Purge));
    for _ in 0..HOLD_STEPS - 1 {
        os.advance_clock(FINE_TICKS);
        back.heartbeat();
    }
    assert_eq!(os.wakes(), 0, "woken before the hold ended");
    assert_eq!(back.retained_large(), freed);
    os.advance_clock(FINE_TICKS);
    back.heartbeat();
    assert_eq!(os.wakes(), 1, "the hold's end woke nobody");
    assert!(!sleeping(&back));
    for _ in 0..8 {
        os.advance_clock(FINE_TICKS);
        back.heartbeat();
    }
    assert_eq!(os.calls(Call::Purge), purges, "a heartbeat returned memory");
    assert_eq!(os.wakes(), 1, "an awake thread was woken again");
    assert!(thread_runs(&back) > 0);
    let left = back.retained_large();
    assert!(
        left <= floor && left + class.size() > floor,
        "down to the floor: {left}"
    );
    back.free(top.ptr, class);
}

/// A thread that planned while free large memory was under the floor is woken by the free
/// whose add takes it over, looks, finds nothing held long enough, and is woken again when the
/// hold of those frees ends.
#[test]
#[cfg_attr(miri, ignore)]
fn a_crossing_over_the_floor_wakes_a_thread_that_planned_under_it() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    asleep(&back);
    back.plan_hold();
    let class = large(256 << 10);
    let floor = allowance_floor(Native::GEOMETRY);
    let top = free_large(&back, os, class, 2 * floor);
    let mut wakes = Vec::new();
    for step in 0..2 * HOLD_STEPS {
        os.advance_clock(FINE_TICKS);
        if beat_and_serve(&back) {
            wakes.push(step);
        }
    }
    assert_eq!(wakes.len(), 2, "woken at steps {wakes:?}");
    assert_eq!(wakes[0], 0, "the crossing's look waited: {wakes:?}");
    assert!(back.retained_large() <= floor);
    back.free(top.ptr, class);
}

/// A clocked pass: the heartbeat that finds it due wakes the thread and takes no step; the
/// thread's steps return what a pass taken by heartbeats returns from the same heap, interval
/// by interval. And with nothing over the floor a due pass is ended by the heartbeat, as one
/// with no thread would end it, and wakes nobody.
#[test]
#[cfg_attr(miri, ignore)]
fn a_pass_is_the_threads_and_returns_what_heartbeats_would() {
    let run = |thread: bool| {
        let state = state::<Standard>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        if thread {
            asleep(&back);
        }
        let class = small(1024);
        let total = retained_cap(Standard::GEOMETRY) / 2;
        let slots = fill(&back, class, total / 1024);
        for &p in &slots {
            paint(p, 1024, 5);
        }
        // SAFETY: the test holds these slots.
        unsafe { back.launder(class, &slots) };
        let mut seen = Vec::new();
        for _ in 0..6 {
            os.advance_clock(DECAY_TICKS);
            let purges = os.calls(Call::Purge);
            let mut woken = 0;
            for _ in 0..BEATS {
                if thread {
                    let before = os.calls(Call::Purge);
                    back.heartbeat();
                    assert_eq!(os.calls(Call::Purge), before, "a heartbeat returned memory");
                    if !sleeping(&back) {
                        woken += 1;
                        thread_runs(&back);
                        asleep(&back);
                    }
                } else {
                    back.heartbeat();
                }
            }
            if thread && os.calls(Call::Purge) > purges {
                assert_eq!(woken, 1, "an interval's pass woke the thread {woken} times");
            }
            seen.push(back.retained());
        }
        seen
    };
    let (slow, thread) = (run(false), run(true));
    assert_eq!(slow, thread, "what is retained after each interval's pass");
    assert!(
        slow.last() < slow.first(),
        "the passes returned nothing: {slow:?}"
    );

    // Under the floor, as the process has it.
    let state = floored_state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    asleep(&back);
    for _ in 0..4 {
        os.advance_clock(DECAY_TICKS);
        back.heartbeat();
        assert!(!pass_under_way(&state));
    }
    assert_eq!(os.wakes(), 0);
}

/// Growth over the cap: a grower takes no payment and wakes the thread once a step of growth;
/// the thread's steps pay what heartbeats and growers would have. Under the cap the debt is
/// dropped and nobody is woken.
#[test]
#[cfg_attr(miri, ignore)]
fn growth_over_the_cap_wakes_the_thread_once_a_step_and_the_thread_pays() {
    let run = |thread: bool| {
        let state = state::<Native>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let freed = large(128 << 10);
        let cap = retained_cap(Native::GEOMETRY);
        let n = 2 * cap / freed.size();
        let blocks: Vec<Block> = (0..n)
            .map(|_| back.alloc(freed, freed.size()).unwrap())
            .collect();
        for b in &blocks {
            paint(b.ptr.as_ptr(), freed.size(), 0x33);
            back.free(b.ptr, freed);
        }
        if thread {
            asleep(&back);
        }
        let step = growth_step(Native::GEOMETRY);
        let other = large(64 << 10);
        let mut carved = Vec::new();
        let mut wakes = 0;
        for _ in 0..4 * step / other.size() {
            let purges = os.calls(Call::Purge);
            carved.push(back.alloc(other, other.size()).unwrap());
            if thread {
                assert_eq!(os.calls(Call::Purge), purges, "a grower paid");
                if !sleeping(&back) {
                    wakes += 1;
                    thread_runs(&back);
                    asleep(&back);
                }
            }
        }
        if thread {
            assert_eq!(
                wakes, 4,
                "four steps of growth woke the thread {wakes} times"
            );
        } else {
            // What the growers left owed, the heartbeats pay.
            for _ in 0..64 {
                back.heartbeat();
            }
        }
        back.retained()
    };
    let (slow, thread) = (run(false), run(true));
    let cap = retained_cap(Native::GEOMETRY);
    assert_eq!(slow, thread);
    assert_eq!(slow, 2 * cap - 4 * growth_step(Native::GEOMETRY));

    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    asleep(&back);
    let other = large(64 << 10);
    for _ in 0..8 * growth_step(Native::GEOMETRY) / other.size() {
        back.alloc(other, other.size()).unwrap();
    }
    assert_eq!(os.wakes(), 0, "growth under the cap woke the thread");
    assert!(
        back.state
            .growth
            .load(core::sync::atomic::Ordering::Relaxed)
            < growth_step(Native::GEOMETRY)
    );
}

/// A thread that does not run (awake but never scheduled, so its word is never marked asleep
/// and nobody wakes it): growers leave growth unpaid up to the limit, and past it each pays a
/// step itself, so what is retained over the cap stays within the limit and a step or so.
#[test]
#[cfg_attr(miri, ignore)]
fn growth_past_the_limit_is_paid_by_the_grower_when_the_thread_does_not_run() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let freed = large(128 << 10);
    let cap = retained_cap(Native::GEOMETRY);
    let n = 4 * cap / freed.size();
    let blocks: Vec<Block> = (0..n)
        .map(|_| back.alloc(freed, freed.size()).unwrap())
        .collect();
    for b in &blocks {
        paint(b.ptr.as_ptr(), freed.size(), 0x33);
        back.free(b.ptr, freed);
    }
    back.state.thread.set(KEY | AWAKE);
    let limit = growth_unpaid(Native::GEOMETRY);
    let step = growth_step(Native::GEOMETRY);
    let other = large(64 << 10);
    let purges = os.calls(Call::Purge);
    let mut grown = 0;
    while grown < limit {
        back.alloc(other, other.size()).unwrap();
        grown += other.size();
    }
    assert_eq!(os.calls(Call::Purge), purges, "paid before the limit");
    let mut most_unpaid = 0;
    for _ in 0..4 * limit / other.size() {
        back.alloc(other, other.size()).unwrap();
        grown += other.size();
        let paid = 4 * cap - back.retained();
        most_unpaid = most_unpaid.max(grown - paid);
    }
    assert!(os.calls(Call::Purge) > purges, "nobody paid past the limit");
    assert!(
        most_unpaid <= limit + step,
        "growth left unpaid: {most_unpaid}"
    );
    assert_eq!(os.wakes(), 0);
}
