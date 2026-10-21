//! Failing tests from the review of `burst-start` (counted intervals, purging by runs,
//! `maintain` limiting its own rate).
//!
//! Both model a process in which `maintain` is the only thing that runs: a caller every half
//! interval, as the background thread and the tokio task are, with the round's own work taking
//! half a millisecond of the clock between its one clock read (`depot::heartbeat::Clock`
//! caches it) and the end of its purge pass, which stamps the decay word with a fresh read.
//! The simulated clock does not move by itself, which is why the tests that run `maintain`
//! over the simulated heap do not see this.

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
