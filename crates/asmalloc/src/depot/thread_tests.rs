//! The background thread's word: the slow path's wake, and parking without losing a wake-up
//! (`thread`, `crate::thread_word`).

use super::*;
use crate::depot::Cache;
use crate::depot::thread::{Rest, Seen};
use crate::thread_word::{AWAKE, DORMANT, NONE, PARKED, WANTED};

/// The rounds the thread takes, each an interval on: until it parks, at most `most`; how many.
fn rounds_until_parked<O: Os, R: Routines, S: Source>(
    rig: &TestRig,
    cache: &Cache<'_, O, R, S>,
    seen: &mut Seen,
    most: usize,
) -> usize {
    for round in 1..=most {
        rig.os().advance_clock(IDLE_TICKS);
        let mut work = false;
        loop {
            let step = cache.maintain_step_with_work();
            work |= step.work;
            if !step.more {
                break;
            }
        }
        if cache.rest(seen, work) == Rest::Park {
            return round;
        }
    }
    panic!("the thread did not park in {most} rounds");
}

/// A slow path on CPU 0, so that the magazine area and the drain are ready.
fn one_slow_path(rig: &TestRig) {
    let user = SimCpu::new(0);
    let program = rig.cache(&user);
    let object = program.alloc(tiny_class()).unwrap();
    program.free(tiny_class(), object);
}

/// The case parking must not break: a program whose CPU loaded a magazine before the silence
/// and resumes with swings smaller than a magazine, which would stay on its fast paths for
/// ever. The thread parks only after its rounds drained that CPU's slab, so the first call
/// after the silence takes a slow path, and that slow path wakes the thread.
#[test]
fn a_parked_thread_is_woken_by_a_cpu_that_resumes_on_small_swings() {
    let rig = rig(12, TIGHT);
    let user = SimCpu::new(0);
    let program = rig.cache(&user);
    let class = tiny_class();
    let swing = |n: usize| {
        let held: Vec<_> = (0..n).filter_map(|_| program.alloc(class)).collect();
        for object in held {
            program.free(class, object);
        }
    };
    swing(3);
    assert!(!cpu_0_is_empty(&rig));
    let word = &rig.source.thread;
    assert!(word.claim_start(rig.os(), AWAKE));
    let keeper = SimCpu::new(1);
    let thread = rig.cache(&keeper);
    let mut seen = Seen::default();
    rounds_until_parked(&rig, &thread, &mut seen, 6);
    assert_eq!(word.state.load(Relaxed), PARKED);
    assert!(cpu_0_is_empty(&rig), "parked with a loaded slab");
    assert_eq!(rig.os().wakes(), 0);
    swing(1);
    assert_eq!(
        rig.os().wakes(),
        1,
        "the resuming CPU's slow path did not wake the thread"
    );
    assert_eq!(word.state.load(Relaxed), AWAKE);
    swing(1);
    assert_eq!(rig.os().wakes(), 1, "a slow path woke an awake thread");
}

/// The thread does not park while the heap is over the floor, or where drains are off.
#[test]
fn the_thread_parks_only_on_a_quiet_heap_under_the_floor_with_drains() {
    let rig = rig(12, TIGHT);
    one_slow_path(&rig);
    rig.source.over_floor.store(true, Relaxed);
    assert!(rig.source.thread.claim_start(rig.os(), AWAKE));
    let keeper = SimCpu::new(1);
    let thread = rig.cache(&keeper);
    let mut seen = Seen::default();
    for _ in 0..4 {
        rig.os().advance_clock(IDLE_TICKS);
        thread.maintain();
        assert_eq!(thread.rest(&mut seen, false), Rest::Sleep);
    }
    rig.source.over_floor.store(false, Relaxed);
    assert_eq!(rounds_until_parked(&rig, &thread, &mut seen, 2), 1);

    let rig = super::rig(12, TIGHT);
    rig.os().set_faults(Call::RegisterFence, Faults::ALWAYS);
    one_slow_path(&rig);
    assert!(rig.source.thread.claim_start(rig.os(), AWAKE));
    let thread = rig.cache(&keeper);
    let mut seen = Seen::default();
    for _ in 0..4 {
        rig.os().advance_clock(IDLE_TICKS);
        thread.maintain();
        assert_eq!(
            thread.rest(&mut seen, false),
            Rest::Sleep,
            "parked without drains"
        );
    }
}

/// A heap the thread has drained and found quiet, ready for its look before parking: binds the
/// rig, the thread's cache and what it has seen.
macro_rules! ready_to_park {
    ($rig:ident, $thread:ident, $seen:ident) => {
        let $rig = rig(12, TIGHT);
        let keeper = SimCpu::new(1);
        let $thread = $rig.cache(&keeper);
        one_slow_path(&$rig);
        assert!($rig.source.thread.claim_start($rig.os(), AWAKE));
        let mut $seen = Seen::default();
        for _ in 0..4 {
            $rig.os().advance_clock(IDLE_TICKS);
            $thread.maintain();
            let _ = $thread.first_look(&mut $seen, false);
        }
        assert!(cpu_0_is_empty(&$rig));
    };
}

/// A slow path whose load of the word came before the thread stored `PARKED`: its exchange and
/// its count came before the fence too, so the thread's second look sees them and it backs out.
#[test]
fn a_slow_path_before_the_store_is_seen_by_the_second_look() {
    ready_to_park!(rig, thread, seen);
    let activity = thread.first_look(&mut seen, false).unwrap();
    // The slow path, all of it before the store: it finds the word awake and wakes nobody.
    let user = SimCpu::new(0);
    let program = rig.cache(&user);
    let object = program.alloc(tiny_class()).unwrap();
    assert_eq!(rig.os().wakes(), 0);
    assert!(thread.mark_parked());
    assert_eq!(thread.second_look(activity), Rest::Sleep);
    assert_eq!(rig.source.thread.state.load(Relaxed), AWAKE);
    program.free(tiny_class(), object);
}

/// A slow path whose load came after the store finds `PARKED` and wakes the thread; the thread's
/// second look backs out or finds the word already moved, and does not wait.
#[test]
fn a_slow_path_after_the_store_wakes_the_thread() {
    ready_to_park!(rig, thread, seen);
    let activity = thread.first_look(&mut seen, false).unwrap();
    assert!(thread.mark_parked());
    let user = SimCpu::new(0);
    let program = rig.cache(&user);
    let object = program.alloc(tiny_class()).unwrap();
    assert_eq!(rig.os().wakes(), 1);
    assert_eq!(rig.source.thread.state.load(Relaxed), AWAKE);
    assert_eq!(thread.second_look(activity), Rest::Sleep);
    assert_eq!(rig.source.thread.state.load(Relaxed), AWAKE);
    program.free(tiny_class(), object);
}

/// A crossing of the floor wants a thread on a thread that is not the process's first, and
/// wakes a dormant one; a slow path's wake answers `PARKED` only, not `DORMANT`; and one claim
/// creates the thread, a second finding it running.
#[test]
fn a_crossing_wants_or_wakes_the_thread_and_one_claim_creates_it() {
    let rig = rig(12, TIGHT);
    let word = &rig.source.thread;
    word.crossed(rig.os());
    assert_eq!(
        word.state.load(Relaxed),
        NONE,
        "on the process's first thread"
    );
    rig.os().set_thread_id(2);
    word.crossed(rig.os());
    assert_eq!(word.state.load(Relaxed), WANTED);
    assert!(word.claim_start(rig.os(), DORMANT));
    assert!(!word.claim_start(rig.os(), DORMANT));
    one_slow_path(&rig);
    assert_eq!(rig.os().wakes(), 0, "a slow path woke a dormant thread");
    word.crossed(rig.os());
    assert_eq!(word.state.load(Relaxed), AWAKE);
    assert_eq!(rig.os().wakes(), 1);
    assert!(!word.claim_start(rig.os(), AWAKE), "created twice");
}
