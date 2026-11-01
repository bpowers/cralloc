//! The floor measured against what a purge could return, and frontiers that a silence does not
//! age (`purge`, "The floor" and "Frontiers age while the program runs";
//! `docs/research/default-thread-servers.md`, "The burst after a silence, diagnosed").
//!
//! A silence is modelled as the background thread makes one: `maintain`'s steps of the clocked
//! pass (`Back::decay_pass_step`), one round an interval, and no heartbeat.

use super::*;
use crate::back::policy::{
    PURGE_ANY_RUN_EVERY, PURGE_MAX_RUN_PAGES, allowance_floor, retained_cap,
};

/// One interval of silence: the clock moves an interval and `maintain`'s round runs the
/// clocked pass to its end, as the thread's round does, with no heartbeat.
fn quiet_round<S: Shape>(back: &Back<'_, Sim<S>>) {
    back.space().os().advance_clock(DECAY_TICKS);
    let now = back.now();
    while back.decay_pass_step(&|| now).more {}
}

/// Frees every slot of `class` in `slots` but one a page: memory the retained count holds that
/// no purge can return.
fn fragment<O: Os>(back: &Back<'_, O>, class: SmallClass, slots: &[*mut u8]) {
    let per_page = O::GEOMETRY.page_size() / class.size();
    let freed: Vec<*mut u8> = slots
        .iter()
        .copied()
        .filter(|p| !(p.addr() / class.size()).is_multiple_of(per_page))
        .collect();
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &freed) };
}

/// Found on a 1.4 GiB server: the floor read 185 MiB retained, of which 1.4 MiB could be
/// returned, so it never applied, and passes took the last free pages the next burst faulted
/// back. Over a heap whose retained memory is mostly free slots beside live ones, a little
/// returnable memory under the floor stays, however many passes run, though the retained count
/// is over the floor and over the cap. Once more than the floor could be returned, the passes
/// take it down to the floor.
#[test]
#[cfg_attr(miri, ignore)]
fn the_floor_is_measured_against_what_a_purge_could_return() {
    let state = floored_state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let page = Standard::GEOMETRY.page_size();
    let floor = allowance_floor(Standard::GEOMETRY);
    let cap = retained_cap(Standard::GEOMETRY);
    let pinned = small(1024);
    let pinneds = fill(&back, pinned, whole_regions(&back, pinned, 2 * cap));
    fragment(&back, pinned, &pinneds);
    let loose = small(512);
    let looses = fill(&back, loose, whole_regions(&back, loose, 2 * floor));
    for &p in &looses {
        paint(p, 512, 7);
    }
    let (under, over) = looses.split_at(floor / 2 / 512);
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(loose, under) };
    forget_reuse(&back);
    let retained = back.retained();
    assert!(retained > cap, "{retained} retained");
    for _ in 0..2 * PURGE_ANY_RUN_EVERY {
        os.advance_clock(DECAY_TICKS);
        beat(&back);
    }
    assert_eq!(
        os.calls(Call::Purge),
        0,
        "a pass took memory under the floor"
    );
    assert_eq!(back.retained(), retained);
    // SAFETY: as above.
    unsafe { back.launder(loose, over) };
    forget_reuse(&back);
    for _ in 0..2 * PURGE_ANY_RUN_EVERY {
        os.advance_clock(DECAY_TICKS);
        beat(&back);
    }
    let kept = back.retained() - (retained - floor / 2);
    let run = PURGE_MAX_RUN_PAGES as usize * page;
    assert!(
        kept + run >= floor && kept <= floor + run,
        "{kept} bytes of returnable memory kept against a floor of {floor}"
    );
}

/// Only an interval in which a heartbeat ran ages a frontier. Through a silence of many
/// intervals in which only `maintain` runs, the interval count stands, the frontier of the last
/// fills holds, and memory under the floor stays; the first heartbeat after the silence counts
/// one interval, so the frontier still holds for the burst. With the frontiers aged by the
/// thread's rounds, the passes of the silence took it all.
#[test]
#[cfg_attr(miri, ignore)]
fn a_silence_ages_no_frontier() {
    let state = floored_state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let floor = allowance_floor(Standard::GEOMETRY);
    let class = small(1024);
    beat(&back);
    let slots = fill(&back, class, floor / 2 / 1024);
    for &p in &slots {
        paint(p, 1024, 5);
    }
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &slots) };
    let (epoch, frontier) = (back.epoch(), floor_at(&back, slots[0]));
    assert!(frontier > 0);
    for _ in 0..4 * PURGE_ANY_RUN_EVERY {
        quiet_round(&back);
    }
    assert_eq!(back.epoch(), epoch, "a silence counted intervals");
    assert_eq!(
        floor_at(&back, slots[0]),
        frontier,
        "a silence aged the frontier"
    );
    assert_eq!(
        os.calls(Call::Purge),
        0,
        "a silence took memory under the floor"
    );
    os.advance_clock(DECAY_TICKS);
    beat(&back);
    assert_eq!(back.epoch(), epoch + 1);
    assert_eq!(floor_at(&back, slots[0]), frontier);
    assert_eq!(
        os.calls(Call::Purge),
        0,
        "the burst's first heartbeat purged"
    );
}

/// In a silence the passes go below the frontiers while more than the floor could be returned,
/// and stop at the floor, from the top of the region down: the lowest pages, the ones the next
/// fills take, are what stays. A heartbeat's pass never goes below a frontier.
#[test]
#[cfg_attr(miri, ignore)]
fn in_a_silence_a_pass_goes_below_the_frontiers_down_to_the_floor() {
    let state = floored_state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let page = Standard::GEOMETRY.page_size();
    let floor = allowance_floor(Standard::GEOMETRY);
    let class = small(1024);
    beat(&back);
    let slots = fill(&back, class, whole_regions(&back, class, 3 * floor));
    for &p in &slots {
        paint(p, 1024, 5);
    }
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class, &slots) };
    let retained = back.retained();
    // While the program runs, the fills' frontier keeps every page: the heartbeats' passes
    // count nothing a purge could return and take nothing.
    os.advance_clock(DECAY_TICKS);
    beat(&back);
    assert_eq!(
        os.calls(Call::Purge),
        0,
        "a heartbeat's pass went below the frontier"
    );
    for _ in 0..4 * PURGE_ANY_RUN_EVERY {
        quiet_round(&back);
    }
    let kept = back.retained();
    let run = PURGE_MAX_RUN_PAGES as usize * Standard::GEOMETRY.huge_page_size() / page * page;
    assert!(kept < retained, "the silence returned nothing");
    assert!(
        kept + run >= floor && kept <= floor + run,
        "{kept} bytes kept against a floor of {floor}"
    );
    // SAFETY: no other thread runs, and the back end and this test store only integers.
    unsafe {
        assert!(
            !os.is_purged(NonNull::new(slots[0]).unwrap(), 1024),
            "the lowest page went"
        );
        assert!(os.is_purged(NonNull::new(*slots.last().unwrap()).unwrap(), 1024));
    }
}

/// A lap counts what it saw, and the next pass is held to it: a fragmented heap over the floor
/// by its retained count, with nothing a purge could return, still costs one lap an interval,
/// to count, and no purge call.
#[test]
#[cfg_attr(miri, ignore)]
fn a_heap_with_nothing_returnable_is_counted_and_kept() {
    let state = floored_state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let class = small(2048);
    let floor = allowance_floor(Standard::GEOMETRY);
    let slots = fill(&back, class, whole_regions(&back, class, 2 * floor));
    fragment(&back, class, &slots);
    forget_reuse(&back);
    assert!(back.over_floor());
    let laps = || state.walk.laps.load(core::sync::atomic::Ordering::Relaxed);
    for _ in 0..3 {
        let before = laps();
        quiet_round(&back);
        assert_eq!(laps(), before + 1, "a pass held to nothing walks one lap");
    }
    assert_eq!(back.returnable(), Some(0));
    assert_eq!(os.calls(Call::Purge), 0);
}
