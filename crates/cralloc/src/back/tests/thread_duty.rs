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
        let step = back.thread_purge_steps(&|| back.now());
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

/// A region held for the hold that keeps its stamp, as a look that stopped at its bound of
/// bitmap leaves it, sets the thread's next look at the next step, as a heartbeat would take
/// it, and not at the next stamp's end.
#[test]
#[cfg_attr(miri, ignore)]
fn a_held_region_left_by_a_look_is_looked_at_the_next_step() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    asleep(&back);
    let class = large(256 << 10);
    let top = free_large(&back, os, class, 2 * allowance_floor(Native::GEOMETRY));
    // The hold has passed without a heartbeat to look.
    back.hold_set_count(back.hold_count().wrapping_add(HOLD_STEPS + 3));
    back.plan_hold();
    assert_eq!(os.wakes(), 0);
    os.advance_clock(FINE_TICKS);
    back.heartbeat();
    assert_eq!(os.wakes(), 1, "the next step did not wake the thread");
    assert!(thread_runs(&back) > 0);
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

/// A heap 2 caps of 128 KiB slots freed (over the cap, with no hold pending: the clock has not
/// moved since the frees), and the word as `word`.
fn over_the_cap(state: &State<Sim<Native>>, word: u64) {
    let back = state.get_or_reserve().unwrap();
    let freed = large(128 << 10);
    let blocks: Vec<Block> = (0..2 * retained_cap(Native::GEOMETRY) / freed.size())
        .map(|_| back.alloc(freed, freed.size()).unwrap())
        .collect();
    for b in &blocks {
        paint(b.ptr.as_ptr(), freed.size(), 0x33);
        back.free(b.ptr, freed);
    }
    back.state.thread.set(word);
}

/// The thread's look before it sleeps (`Back::purge_due`), clause by clause: nothing on a
/// heap with nothing to do; growth owed; a pass due over the floor, before any heartbeat
/// starts it; a pass under way; the hold's next look reached, and not in a silence.
#[test]
#[cfg_attr(miri, ignore)]
fn the_threads_look_before_it_sleeps_sees_each_kind_of_work() {
    // Nothing.
    let state = state::<Native>();
    over_the_cap(&state, KEY | AWAKE);
    let back = state.get_or_reserve().unwrap();
    assert!(!back.purge_due(&|| back.now()), "work on a heap with none");
    // Growth owed.
    back.state.growth.store(
        growth_step(Native::GEOMETRY),
        core::sync::atomic::Ordering::SeqCst,
    );
    assert!(back.purge_due(&|| back.now()), "growth owed was not seen");
    back.state
        .growth
        .store(0, core::sync::atomic::Ordering::SeqCst);
    // A pass due, then under way.
    state.os().advance_clock(DECAY_TICKS);
    assert!(back.purge_due(&|| back.now()), "a pass due was not seen");
    assert!(back.heartbeat_for(&|| back.now(), back.state.thread.load()));
    assert!(pass_under_way(&state));
    assert!(
        back.purge_due(&|| back.now()),
        "a pass under way was not seen"
    );
    while back.thread_purge_steps(&|| back.now()).more {}
    assert!(!pass_under_way(&state));
    // The hold's next look.
    back.hold_set_next(super::super::hold::NEXT_AT | u64::from(back.hold_count().wrapping_add(1)));
    assert!(!back.purge_due(&|| back.now()), "a look before its step");
    state.os().advance_clock(FINE_TICKS);
    assert!(
        back.purge_due(&|| back.now()),
        "a look reached was not seen"
    );
    // In a silence the look is the round's.
    state.os().advance_clock(3 * DECAY_TICKS);
    let _ = back.now();
    state.os().advance_clock(DECAY_TICKS);
    // The interval's pass, which a silence does not hold back, first.
    while back.thread_purge_steps(&|| back.now()).more {}
    back.hold_set_next(super::super::hold::NEXT_AT | u64::from(back.hold_count()));
    assert!(
        !back.purge_due(&|| back.now()),
        "a silence's look taken from the round"
    );
}

/// A free that stamps a region when no look is planned sets one a hold ahead.
#[test]
#[cfg_attr(miri, ignore)]
fn a_free_with_no_look_planned_sets_one_a_hold_ahead() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let class = large(256 << 10);
    let a = back.alloc(class, class.size()).unwrap();
    let _top = back.alloc(class, class.size()).unwrap();
    back.hold_set_next(0);
    let count = back.hold_count();
    back.free(a.ptr, class);
    assert_eq!(
        back.hold_next(),
        super::super::hold::NEXT_AT | u64::from(count.wrapping_add(HOLD_STEPS))
    );
}

/// A heartbeat pays growth the thread has left unpaid past the limit, and says the rest is
/// work for the thread; a grower past the limit pays a step and wakes a sleeping thread.
#[test]
#[cfg_attr(miri, ignore)]
fn growth_past_the_limit_is_paid_by_heartbeats_and_wakes_the_thread() {
    let state = state::<Native>();
    over_the_cap(&state, KEY | AWAKE);
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let limit = growth_unpaid(Native::GEOMETRY);
    back.state
        .growth
        .store(2 * limit, core::sync::atomic::Ordering::SeqCst);
    let (purges, retained) = (os.calls(Call::Purge), back.retained());
    assert!(
        back.heartbeat_for(&|| back.now(), back.state.thread.load()),
        "what the payment left was not work for the thread"
    );
    assert!(os.calls(Call::Purge) > purges, "the heartbeat did not pay");
    assert!(back.retained() < retained);
    asleep(&back);
    back.state
        .growth
        .store(limit, core::sync::atomic::Ordering::SeqCst);
    let purges = os.calls(Call::Purge);
    back.alloc(large(64 << 10), 64 << 10).unwrap();
    assert!(
        os.calls(Call::Purge) > purges,
        "the grower past the limit did not pay"
    );
    assert_eq!(
        os.wakes(),
        1,
        "the grower past the limit did not wake the thread"
    );
}

/// A pass under way that the thread does not step, an interval after it began, is the slow
/// paths' again, and growth past the limit is then paid as it is without a pass: the bound
/// does not rest on the scheduler.
#[test]
#[cfg_attr(miri, ignore)]
fn a_pass_the_thread_leaves_is_the_heartbeats_after_an_interval() {
    let state = state::<Native>();
    over_the_cap(&state, KEY | AWAKE);
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    os.advance_clock(DECAY_TICKS);
    back.heartbeat();
    assert!(pass_under_way(&state), "no pass was started");
    // The thread sleeps, and does not come to step the pass.
    asleep(&back);
    let (purges, wakes) = (os.calls(Call::Purge), os.wakes());
    for _ in 0..BEATS {
        back.heartbeat();
    }
    assert_eq!(os.calls(Call::Purge), purges, "stepped before an interval");
    assert_eq!(os.wakes(), wakes, "woken before an interval");
    os.advance_clock(DECAY_TICKS);
    back.heartbeat();
    assert_eq!(
        os.wakes(),
        wakes + 1,
        "the overdue pass did not wake the thread"
    );
    for _ in 0..BEATS {
        back.heartbeat();
    }
    assert!(
        !pass_under_way(&state),
        "the heartbeats did not end the pass"
    );
}

/// The thread's look before it sleeps against a grower, with the real back end: the debt and
/// the word on the checker's atomics. No schedule has the thread sleep on its mark with a step
/// of growth owed over the cap.
#[test]
#[cfg_attr(miri, ignore)]
fn the_threads_sleep_loses_no_growth_under_every_schedule() {
    use crate::front::check::{Bounds, explore};
    use std::sync::Mutex;
    struct World {
        state: Box<State<Sim<Native>>>,
        asleep: Mutex<Option<Option<u64>>>,
    }
    let found = explore(
        Bounds::preemptions(2),
        2,
        || {
            let state = state::<Native>();
            over_the_cap(&state, KEY | AWAKE);
            World {
                state,
                asleep: Mutex::new(None),
            }
        },
        |world, me| {
            let back = world.state.get_or_reserve().unwrap();
            if me == 0 {
                back.alloc(large(64 << 10), 64 << 10).unwrap();
            } else {
                let word = back.state.thread.begin_sleep(KEY);
                let due = back.purge_due(&|| back.now());
                *world.asleep.lock().unwrap() = Some((!due).then_some(word.0));
            }
        },
        |_| {},
        |world, _| {
            let back = world.state.get_or_reserve().unwrap();
            let Some(Some(waits)) = *world.asleep.lock().unwrap() else {
                return;
            };
            if waits as u32 == back.state.thread.load().0 as u32 {
                assert!(
                    back.state.growth.load(core::sync::atomic::Ordering::SeqCst)
                        < growth_step(Native::GEOMETRY),
                    "the thread sleeps with growth owed and nobody to wake it"
                );
            }
        },
    );
    assert!(found.complete, "{found:?}");
}

/// The thread's plan against a free that takes free large memory over the floor: a plan made
/// under the floor must not hide the crossing's look.
#[test]
#[cfg_attr(miri, ignore)]
fn the_threads_plan_keeps_a_crossings_look_under_every_schedule() {
    use crate::front::check::{Bounds, explore};
    struct World {
        state: Box<State<Sim<Native>>>,
        last: Last,
    }
    /// The slot the free gives back, handed to the freeing thread.
    struct Last(Block);
    // SAFETY: the block is only freed, by one thread, and the test reads none of its bytes.
    unsafe impl Sync for Last {}
    let class = large(256 << 10);
    let floor = allowance_floor(Native::GEOMETRY);
    let found = explore(
        Bounds::preemptions(2),
        2,
        || {
            let state = floored_state::<Native>();
            let back = state.get_or_reserve().unwrap();
            let blocks: Vec<Block> = (0..floor / class.size() + 1)
                .map(|_| back.alloc(class, class.size()).unwrap())
                .collect();
            let _top = back.alloc(class, class.size()).unwrap();
            for b in &blocks[1..] {
                back.free(b.ptr, class);
            }
            assert!(back.retained_large() <= floor);
            back.hold_set_next(super::super::hold::NEXT_LEVEL);
            let last = Last(blocks[0]);
            World { state, last }
        },
        |world, me| {
            let back = world.state.get_or_reserve().unwrap();
            if me == 0 {
                back.free(world.last.0.ptr, class);
            } else {
                back.plan_hold();
            }
        },
        |_| {},
        |world, _| {
            let back = world.state.get_or_reserve().unwrap();
            assert!(back.retained_large() > floor);
            assert!(
                back.hold_next() & super::super::hold::NEXT_AT != 0,
                "the plan hid the crossing: next look {:#x}",
                back.hold_next()
            );
        },
    );
    assert!(found.complete, "{found:?}");
}

/// The most growth left unpaid while 32 MiB is carved in 1 MiB slots (two of a slow path's
/// purge steps each, in this geometry) over a heap 40 MiB over the cap, with four heartbeats after each carve (2 MiB of payments where heartbeats pay) and a thread that is awake and never runs.
/// With `pass`, an interval's pass was started by a heartbeat first and left to the thread.
fn most_unpaid(pass: bool) -> usize {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let freed = large(128 << 10);
    let cap = retained_cap(Native::GEOMETRY);
    let blocks: Vec<Block> = (0..20 * cap / freed.size())
        .map(|_| back.alloc(freed, freed.size()).unwrap())
        .collect();
    for b in &blocks {
        paint(b.ptr.as_ptr(), freed.size(), 0x33);
        back.free(b.ptr, freed);
    }
    back.state.thread.set(KEY | AWAKE);
    if pass {
        os.advance_clock(DECAY_TICKS);
        back.heartbeat();
        assert!(pass_under_way(&state), "no pass was started");
    }
    let start = back.retained();
    let other = large(1 << 20);
    let (mut grown, mut most) = (0, 0);
    for carve in 0..32 {
        back.alloc(other, other.size())
            .unwrap_or_else(|| panic!("carve {carve}"));
        grown += other.size();
        for _ in 0..4 {
            back.heartbeat();
        }
        let paid = start.saturating_sub(back.retained());
        most = most.max(grown.saturating_sub(paid));
    }
    assert_eq!(os.wakes(), 0);
    most
}

/// Design, "The background thread": the bound on retained memory does not rest on the scheduler. With a
/// thread that does not run, growth left unpaid stays near the limit also while a clocked pass is
/// under way. A pass that a heartbeat started for the thread is stepped by nobody; were it to stay
/// under way for ever with the heartbeats' payments past the limit off during a pass, only the
/// grower's one step a carve would be left.
#[test]
#[cfg_attr(miri, ignore)]
fn unpaid_growth_is_bounded_when_the_thread_does_not_run_and_a_pass_is_under_way() {
    let limit = growth_unpaid(Native::GEOMETRY);
    let slack = growth_step(Native::GEOMETRY) + (1 << 20);
    let without = most_unpaid(false);
    assert!(
        without <= limit + slack,
        "no pass: {} MiB unpaid",
        without >> 20
    );
    let with = most_unpaid(true);
    assert!(
        with <= limit + slack,
        "a pass under way: {} MiB unpaid, against {} MiB with none",
        with >> 20,
        without >> 20
    );
}

/// `bytes` of slots of the large class `class`, freed while heartbeats run, with one more slot
/// live above them.
fn free_large_slots_held<O: Os>(
    back: &Back<'_, O>,
    os: &SimOs<Native>,
    class: Class,
    bytes: usize,
) {
    let size = class.size();
    let blocks: Vec<Block> = (0..bytes / size)
        .map(|_| back.alloc(class, size).unwrap())
        .collect();
    let _top = back.alloc(class, size).unwrap();
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
}

/// One round of the purge phase of `maintain` in a silence, an interval after the last: by a
/// program's timer (`thread` false), or as `heap::run_thread` takes it, with a purge step of
/// the thread's after every step of the round and the thread's steps after it. The round's
/// earlier steps (the idle looks, the stacks) are the depot's and read no fine count; the
/// thread's purge step after the first of them is the first reading of the hold's clock.
fn quiet_round<O: Os>(back: &Back<'_, O>, os: &SimOs<Native>, thread: bool) {
    use crate::depot::Source;
    os.advance_clock(DECAY_TICKS);
    let now = || back.now();
    if thread {
        let _ = back.thread_purge_steps(&now);
    }
    loop {
        let step = back.purge_pass_step(&now);
        if thread {
            let _ = back.thread_purge_steps(&now);
        }
        if !step.more {
            break;
        }
    }
    if thread {
        while back.thread_purge_steps(&now).more {}
        back.plan_hold();
    }
}

/// Design, "Returning memory and huge pages": "`maintain` takes the same bounded looks once an
/// interval, and in a silence returns held slots at its second round." With the thread's own
/// purge steps between the steps of its round, the thread's look (a heartbeat's, with the
/// whole hold) takes the fine step's one look before `maintain`'s does, in every round, so
/// `maintain`'s shorter hold for a silence is never applied.
#[test]
#[cfg_attr(miri, ignore)]
fn a_quiet_programs_held_slots_go_back_at_the_threads_second_round() {
    let floor = allowance_floor(Native::GEOMETRY);
    let left_after = |thread: bool, rounds: usize| {
        // With the floor, as the process has it, so that the clocked pass, which stops at the
        // floor of all that is retained, is not what returns the slots.
        let state = floored_state::<Native>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        if thread {
            back.state.thread.set(KEY | AWAKE);
        }
        free_large_slots_held(&back, os, large(256 << 10), 2 * floor);
        for _ in 0..rounds {
            quiet_round(&back, os, thread);
        }
        back.retained_large()
    };
    assert!(left_after(false, 2) <= floor, "a timer's second round");
    let left: Vec<usize> = [2, 4, 8]
        .iter()
        .map(|&n| left_after(true, n) >> 10)
        .collect();
    assert!(
        left[0] << 10 <= floor,
        "KiB of free large slots left after the thread's rounds 2, 4 and 8: {left:?}, \
         against a floor of {} KiB",
        floor >> 10
    );
}

/// A region that frees keep young, with the thread running: the thread returns its old free
/// slots by their own ages (`hold::walk_aged`), woken about once a hold and not at every step,
/// and no heartbeat returns anything itself. Once the program has gone quiet, the thread's
/// rounds return the rest and leave no look planned, so nothing wakes it for the hold.
#[test]
#[cfg_attr(miri, ignore)]
fn the_thread_ages_a_young_regions_slots_and_plans_nothing_once_quiet() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    asleep(&back);
    back.state
        .hold
        .unfloored
        .store(true, core::sync::atomic::Ordering::Relaxed);
    let class = large(64 << 10);
    let size = class.size();
    let blocks: Vec<NonNull<u8>> = (0..16)
        .map(|_| back.alloc(class, size).unwrap().ptr)
        .collect();
    for b in &blocks {
        paint(b.as_ptr(), size, 7);
    }
    for _ in 0..2 {
        os.advance_clock(FINE_TICKS);
        beat_and_serve(&back);
    }
    for b in &blocks[1..] {
        back.free(*b, class);
    }
    let (mut wakes, mut back_at) = (0, None);
    for step in 0..3 * HOLD_STEPS {
        back.free(blocks[0], class);
        assert_eq!(back.alloc(class, size).unwrap().ptr, blocks[0]);
        paint(blocks[0].as_ptr(), size, 7);
        let purges = os.calls(Call::Purge);
        os.advance_clock(FINE_TICKS);
        back.heartbeat();
        assert_eq!(os.calls(Call::Purge), purges, "a heartbeat returned memory");
        if !sleeping(&back) {
            wakes += 1;
            thread_runs(&back);
            asleep(&back);
        }
        // SAFETY: free slots, which nothing else uses meanwhile.
        if back_at.is_none() && unsafe { os.is_purged(blocks[1], size) } {
            back_at = Some(step);
        }
    }
    let back_at = back_at.expect("the old slots never went back");
    assert!(back_at + 3 >= HOLD_STEPS, "returned at step {back_at}");
    assert!(blocks[1..].iter().all(|b| {
        // SAFETY: as above.
        unsafe { os.is_purged(*b, size) }
    }));
    assert!(wakes <= 4, "woken {wakes} times in three holds");
    // The program goes quiet, its last buffer freed.
    back.free(blocks[0], class);
    back.state.thread.set(KEY | AWAKE);
    for _ in 0..4 {
        quiet_round(&back, os, true);
    }
    assert_eq!(back.retained_large(), 0);
    assert_eq!(
        back.hold_next() & super::super::hold::NEXT_AT,
        0,
        "a look planned in a silence"
    );
}
