//! Tests from the review of `burst-start` (counted intervals, purging by runs, `maintain`
//! limiting its own rate) and of its fixes: when a clocked pass is due, its steps, and the
//! floor of the allowance.
//!
//! The first two model a process in which `maintain` is the only thing that runs: a caller
//! every half interval, as the background thread and the tokio task are, with the round's own
//! work taking half a millisecond of the clock between its one clock read
//! (`depot::heartbeat::Clock` caches it) and the end of its purge pass, which stamps the decay
//! word with a fresh read. The simulated clock does not move by itself, which is why the tests
//! that run `maintain` over the simulated heap did not see it.

use super::*;
use core::sync::atomic::Ordering::Relaxed;

/// What a round of `maintain` takes between reading the clock and ending its pass: 2^21 ticks,
/// half a millisecond at 4.3 GHz. A pass that returns tens of megabytes takes several.
const WORK: u64 = 1 << 21;

/// One call of `maintain` as the back end sees it (`Cache::maintain`): the claim of the round,
/// one cached clock read, and the pass to its end. Returns whether a round ran and whether it
/// started (or ended as it started) a pass.
fn maintain_call(state: &State<Sim>, back: &Back<'_, Sim>) -> (bool, bool) {
    let os = state.os();
    if !back.clock().claim_round(os) {
        return (false, false);
    }
    let cached = back.now();
    os.advance_clock(WORK);
    let before = state.decay.word.load(Relaxed);
    back.decay_pass(&|| cached);
    (true, state.decay.word.load(Relaxed) != before)
}

/// With `maintain` alone running, each round runs the clocked pass: a pass an interval. As
/// built, a round runs at the count's step and the last pass ended a little after the step
/// before, so the decay word is never an interval old at the next round, and no later call in
/// that interval is let in: a pass every other interval.
#[test]
fn maintain_alone_runs_a_pass_every_interval() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = small(1024);
    let per_page = Standard::GEOMETRY.page_size() / 1024;
    let slots = fill(&back, class, 2560 * per_page);
    for &p in &slots {
        paint(p, 1024, 9);
    }
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &slots) };
    forget_reuse(&back);
    let (mut rounds, mut passes) = (0, 0);
    for _ in 0..40 {
        os.advance_clock(DECAY_TICKS / 2);
        let (round, pass) = maintain_call(&state, &back);
        rounds += usize::from(round);
        passes += usize::from(pass);
    }
    assert!(rounds >= 18, "{rounds} rounds in 20 intervals");
    assert!(
        passes + 2 >= rounds,
        "{passes} passes in {rounds} rounds of maintain"
    );
}

/// Single free pages go back within a few times `PURGE_ANY_RUN_EVERY` intervals whenever the
/// program went quiet. As built, the pass that takes any run is the one that starts in an
/// interval whose number divides by it, and passes that come every other interval (above)
/// start only in odd intervals or only in even ones: begun on the wrong one, they never take a
/// short run.
#[test]
fn maintain_alone_returns_single_free_pages_whenever_it_began() {
    use super::super::policy::PURGE_ANY_RUN_EVERY;
    for phase in 0..2 {
        let state = state::<Standard>();
        let back = state.get_or_reserve().unwrap();
        let os = state.os();
        let class = small(1024);
        let per_page = Standard::GEOMETRY.page_size() / 1024;
        let pages = small_part(&back, class) / per_page;
        let slots = fill(&back, class, pages * per_page);
        for &p in &slots {
            paint(p, 1024, 9);
        }
        let freed: Vec<*mut u8> = slots
            .chunks(per_page)
            .step_by(2)
            .flatten()
            .copied()
            .collect();
        // SAFETY: the test holds these slots and gives them up.
        unsafe { back.launder(class, &freed) };
        forget_reuse(&back);
        tick(&back, phase);
        let retained = back.retained();
        assert!(retained > 0);
        for _ in 0..2 * 4 * PURGE_ANY_RUN_EVERY {
            os.advance_clock(DECAY_TICKS / 2);
            maintain_call(&state, &back);
        }
        assert_eq!(
            back.retained(),
            0,
            "begun {phase} intervals on, {retained} bytes of single free pages are still \
             resident after {} intervals of maintain",
            4 * PURGE_ANY_RUN_EVERY
        );
    }
}

/// Heartbeats an interval or two apart, each a short run of them as a slow path makes, start a
/// pass at each run of them: with passes due by the interval's number, a pass that ended a
/// little after its interval began does not hold the next one back. As first built, beats an
/// interval apart started a pass every other time.
#[test]
fn heartbeats_an_interval_or_two_apart_start_a_pass_each_time() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = small(1024);
    let per_page = Standard::GEOMETRY.page_size() / 1024;
    let slots = fill(&back, class, 2560 * per_page);
    for &p in &slots {
        paint(p, 1024, 9);
    }
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &slots) };
    forget_reuse(&back);
    let (mut idle, mut missed) = (0, 0);
    for visit in 0..40 {
        os.advance_clock(DECAY_TICKS * (1 + visit % 2));
        if back.retained() == 0 {
            break;
        }
        let before = state.decay.word.load(Relaxed);
        let passes = state.walk.passes.load(Relaxed);
        // One beat: its clock read, cached as `depot::heartbeat::Clock` caches it, then the
        // clock moving under its work, so that a pass it ends ends on a fresh, later reading.
        let cached = back.now();
        os.advance_clock(WORK);
        back.heartbeat_with(&|| cached);
        if before & 1 == 0 {
            idle += 1;
            missed += usize::from(state.walk.passes.load(Relaxed) == passes);
        }
    }
    assert!(idle >= 3, "only {idle} beats found no pass under way");
    assert_eq!(
        missed, 0,
        "{missed} of {idle} beats between passes started none"
    );
}

/// The clocked pass taken a step at a time, as `maintain`'s steps take it: a pass over a large
/// retained heap is many steps, none of which makes more purge calls than its budget of bitmap
/// words allows or purges more than its pages and one run past them, and the pass ends without
/// a second starting in the interval.
#[test]
fn the_pass_in_steps_is_bounded_steps() {
    use super::super::policy::{
        PURGE_MAINTAIN_STEP_PAGES, PURGE_MAINTAIN_STEP_WORDS, PURGE_MAX_RUN_PAGES,
    };
    let page = Standard::GEOMETRY.page_size();
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = small(1024);
    let per_page = Standard::GEOMETRY.page_size() / 1024;
    let slots = fill(&back, class, 2560 * per_page);
    for &p in &slots {
        paint(p, 1024, 9);
    }
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &slots) };
    forget_reuse(&back);
    let retained = back.retained();
    os.advance_clock(DECAY_TICKS);
    let now = back.now();
    let (mut steps, mut most, mut widest) = (0, 0, 0);
    loop {
        let (calls, held) = (os.calls(Call::Purge), back.retained());
        let more = back.decay_pass_step(&|| now).more;
        steps += 1;
        most = most.max(os.calls(Call::Purge) - calls);
        widest = widest.max(held.saturating_sub(back.retained()) / page);
        if !more {
            break;
        }
    }
    assert!(steps > 1, "a pass over {retained} bytes took one step");
    assert!(
        most <= PURGE_MAINTAIN_STEP_WORDS as u64,
        "a step made {most} purge calls"
    );
    assert!(
        widest > 0 && widest < (PURGE_MAINTAIN_STEP_PAGES + PURGE_MAX_RUN_PAGES) as usize,
        "a step purged {widest} pages"
    );
    assert!(back.retained() < retained, "the steps purged nothing");
    assert!(
        !back.decay_pass_step(&|| now).more,
        "a second pass in the interval"
    );
}

/// The floor of the allowance (`policy::allowance_floor`): a heap whose retained memory is
/// under it keeps it through any silence, with no purge call at all.
#[test]
fn a_silence_under_the_floor_returns_nothing() {
    use super::super::policy::allowance_floor;
    let state = floored_state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = small(1024);
    let page = Standard::GEOMETRY.page_size();
    let floor = allowance_floor(Standard::GEOMETRY);
    let slots = fill(&back, class, floor / 2 / 1024);
    for &p in &slots {
        paint(p, 1024, 9);
    }
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &slots) };
    forget_reuse(&back);
    let (retained, calls) = (back.retained(), os.calls(Call::Purge));
    assert!(retained >= floor / 2 - page);
    for _ in 0..4 * super::super::policy::PURGE_ANY_RUN_EVERY {
        os.advance_clock(DECAY_TICKS);
        beat(&back);
        let now = back.now();
        while back.decay_pass_step(&|| now).more {}
    }
    assert_eq!(back.retained(), retained);
    assert_eq!(
        os.calls(Call::Purge),
        calls,
        "a pass under the floor purged"
    );
}

/// Above the floor the clocked passes take retained memory down to the floor and stop there,
/// within a run; `trim`, which `collect` and `malloc_trim` call, still returns all of it.
#[test]
fn the_passes_stop_at_the_floor_and_trim_goes_below_it() {
    use super::super::policy::{PURGE_ANY_RUN_EVERY, PURGE_MAX_RUN_PAGES, allowance_floor};
    let state = floored_state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = small(1024);
    let page = Standard::GEOMETRY.page_size();
    let floor = allowance_floor(Standard::GEOMETRY);
    let slots = fill(&back, class, 4 * floor / 1024);
    for &p in &slots {
        paint(p, 1024, 9);
    }
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &slots) };
    forget_reuse(&back);
    assert!(back.retained() > 3 * floor);
    for _ in 0..4 * PURGE_ANY_RUN_EVERY {
        os.advance_clock(DECAY_TICKS);
        beat(&back);
    }
    let retained = back.retained();
    let run = PURGE_MAX_RUN_PAGES as usize * page;
    assert!(
        retained + run >= floor && retained <= floor + run,
        "{retained} bytes retained against a floor of {floor}"
    );
    back.trim();
    assert_eq!(back.retained(), 0, "trim left memory the floor would keep");
}

/// Second review. A pass whose goal has one to three pages left must still take them from a
/// long run, or stop. As built, a run is cut to what the goal has left before the test for
/// short runs, so every run the walk meets from then on is cut to under
/// `PURGE_MIN_RUN_PAGES`, counted as a short run and stepped over: the pass walks the rest of
/// its lap past long free runs, purging nothing, and ends only when it has gone round.
#[test]
fn a_pass_a_few_pages_short_of_its_goal_does_not_step_over_long_runs() {
    use super::super::policy::decay_goal;
    use crate::classes::MagazineSize;
    use crate::os::sim::Shape;
    use crate::space::Geometry;
    /// As in `the_tail_of_a_run_the_cap_cut_goes_with_it`: a small-page part of 256 pages and
    /// a pass's goal of at least a huge page, 128 pages.
    enum Wide {}
    impl Shape for Wide {
        const GEOMETRY: Geometry = Geometry::new(12, 19, 22, 19, 256, 1, MagazineSize::MAX);
    }
    let state = state::<Wide>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = small(1024);
    let page = Wide::GEOMETRY.page_size();
    let per_page = page / 1024;
    let pages = small_part(&back, class) / per_page;
    let slots = fill(&back, class, pages * per_page);
    for &p in &slots {
        paint(p, 1024, 9);
    }
    // From the top down: a live page, a free run two pages short of the goal, a live page,
    // and a free run of 40 pages.
    let (upper, lower) = (126, 40);
    assert!(pages > upper + lower + 3, "{pages}");
    let top = pages - 1;
    let high = &slots[(top - upper) * per_page..top * per_page];
    let low = &slots[(top - upper - 1 - lower) * per_page..(top - upper - 1) * per_page];
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, high) };
    // SAFETY: as above.
    unsafe { back.launder(class, low) };
    forget_reuse(&back);
    let retained = back.retained();
    assert_eq!(retained, (upper + lower) * page);
    let goal = decay_goal(Wide::GEOMETRY, retained);
    assert_eq!(goal, (upper + 2) * page);
    os.advance_clock(DECAY_TICKS);
    beat(&back);
    assert!(!pass_under_way(&state));
    let purged = retained - back.retained();
    assert!(
        purged >= goal,
        "the pass purged {} pages of a goal of {} and walked past a free run of {lower}",
        purged / page,
        goal / page
    );
}

/// The add that takes the retained count over the floor asks for a look at the level
/// (`crate::thread_word`): laundering up to the floor asks nothing, the launder that crosses it
/// sets the level bit, and a slow path's look then wants a thread if the program has one of its
/// own. Under the floor, the clocked pass's look asks nothing; over it, it asks again.
#[test]
fn the_add_that_crosses_the_floor_asks_for_a_look_at_the_level() {
    use super::super::policy::allowance_floor;
    use crate::thread_word::{NONE, Process, Snapshot, WANTED};
    let state = floored_state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let class = small(1024);
    let floor = allowance_floor(Standard::GEOMETRY);
    let slots = fill(&back, class, 2 * floor / 1024);
    let (under, over) = slots.split_at(floor / 1024);
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, under) };
    assert!(!back.over_floor());
    assert_eq!(back.thread().load(), Snapshot(0));
    // SAFETY: as above.
    unsafe { back.launder(class, over) };
    assert!(back.over_floor());
    assert!(back.thread().load().level());
    let key = 1 << 40;
    let seen = back
        .thread()
        .attend(state.os(), Process { key, forked: false });
    assert_eq!(seen, Snapshot(key | NONE), "no thread of the program's own");
    state.os().set_threads(true);
    // The look once an interval asks again while the heap stays over the floor.
    state.os().advance_clock(2 * DECAY_TICKS);
    back.heartbeat();
    assert!(back.thread().load().level());
    let seen = back
        .thread()
        .attend(state.os(), Process { key, forked: false });
    assert_eq!(seen, Snapshot(key | WANTED));
}
