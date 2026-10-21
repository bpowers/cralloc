//! The background thread's word: the slow path's look, the thread's rule for parking at its
//! real cadence, and parking without losing a wake-up (`thread`, `crate::thread_word`).

use super::*;
use crate::depot::Cache;
use crate::depot::thread::{QUIET_ROUNDS, Rest, Seen};
use crate::thread_word::{AWAKE, DORMANT, LEVEL, NONE, OFF, PARKED, Snapshot, WANTED};

/// One call of the thread's loop: the steps of the round of this interval, if one is due, and
/// the decision after it. Returns the decision and whether a round ran.
fn one_call<O: Os, R: Routines, S: Source>(
    cache: &Cache<'_, O, R, S>,
    seen: &mut Seen,
    key: u64,
) -> (Rest, bool) {
    let mut round = None;
    while let Some(step) = cache.maintain_step_with_work() {
        round = Some(round.unwrap_or(false) | step.work);
        if !step.more {
            break;
        }
    }
    (cache.rest(seen, key, round), round.is_some())
}

/// The thread's loop at its real cadence, `per_interval` calls an interval of the clock, until
/// it parks: how many rounds ran. Panics after `most` intervals.
fn rounds_until_parked<O: Os, R: Routines, S: Source>(
    rig: &TestRig,
    cache: &Cache<'_, O, R, S>,
    seen: &mut Seen,
    per_interval: u64,
    most: u64,
) -> u64 {
    let key = cache.thread_key();
    let mut rounds = 0;
    for _ in 0..most * per_interval {
        let (rest, ran) = one_call(cache, seen, key);
        rounds += u64::from(ran);
        if rest == Rest::Park {
            return rounds;
        }
        rig.os().advance_clock(IDLE_TICKS / per_interval);
    }
    panic!("the thread did not park in {most} intervals");
}

/// A slow path on CPU 0, so that the magazine area and the drain are ready.
fn one_slow_path(rig: &TestRig) {
    let user = SimCpu::new(0);
    let program = rig.cache(&user);
    let object = program.alloc(tiny_class()).unwrap();
    program.free(tiny_class(), object);
}

/// The end of a slow path on CPU 0, whatever its magazines: a large call's heartbeat that
/// does not beat, which counts itself and then looks at the word as every slow path does.
fn slow_path_end(rig: &TestRig) {
    let user = SimCpu::new(0);
    rig.cache(&user).lone_heartbeat();
}

/// The thread's word, awake, as `start_background` leaves it, for the thread on CPU 1.
fn started(rig: &TestRig) -> u64 {
    let keeper = SimCpu::new(1);
    let key = rig.cache(&keeper).thread_key();
    assert_ne!(key, 0);
    assert!(rig.source.thread.claim_start(rig.os(), key, AWAKE));
    key
}

/// The count of quiet rounds: work or a moved counter sets it back to zero, and the first
/// round, with nothing to compare, counts as moved.
#[test]
fn work_or_a_slow_path_starts_the_count_of_quiet_rounds_again() {
    let mut seen = Seen::default();
    assert_eq!(seen.after_round(5, false), 0);
    assert_eq!(seen.after_round(5, false), 1);
    assert_eq!(seen.after_round(5, false), 2);
    assert_eq!(seen.after_round(5, true), 0);
    assert_eq!(seen.after_round(5, false), 1);
    assert_eq!(seen.after_round(6, false), 0);
    assert_eq!(seen.after_round(6, false), 1);
}

/// The thread at its real cadence (it sleeps half an interval or less, so most of its calls
/// find the round of their interval done) parks only after `QUIET_ROUNDS` rounds that ran, not
/// calls, and only once the depot has shed everything the drain left: every object is back.
/// Review finding 5: the thread parked before the round that would have shed the depot, at two
/// and four calls an interval.
#[test]
fn the_thread_parks_after_quiet_rounds_with_the_depot_shed_at_any_cadence() {
    for per_interval in [1, 2, 4] {
        let rig = rig(12, TIGHT);
        park_on_cpu_0(&rig, 8);
        started(&rig);
        let keeper = SimCpu::new(1);
        let thread = rig.cache(&keeper);
        let mut seen = Seen::default();
        let rounds = rounds_until_parked(&rig, &thread, &mut seen, per_interval, 64);
        assert!(
            rounds > QUIET_ROUNDS,
            "{per_interval} calls an interval: parked after {rounds} rounds"
        );
        assert_eq!(
            rig.source.held().len(),
            12,
            "{per_interval} calls an interval: parked with objects in the depot"
        );
        assert!(cpu_0_is_empty(&rig));
        assert_eq!(rig.source.thread.load().state(), PARKED);
    }
}

/// A round whose purge returned memory is work: the thread parks `QUIET_ROUNDS` rounds after
/// the last such round, whatever the heap still holds.
#[test]
fn a_round_that_returns_memory_starts_the_count_again() {
    let rig = rig(12, TIGHT);
    one_slow_path(&rig);
    started(&rig);
    let keeper = SimCpu::new(1);
    let thread = rig.cache(&keeper);
    let mut seen = Seen::default();
    let quiet = rounds_until_parked(&rig, &thread, &mut seen, 2, 64);

    let rig = super::rig(12, TIGHT);
    one_slow_path(&rig);
    started(&rig);
    let thread = rig.cache(&keeper);
    rig.source
        .returning
        .store(5, core::sync::atomic::Ordering::Relaxed);
    let mut seen = Seen::default();
    let busy = rounds_until_parked(&rig, &thread, &mut seen, 2, 64);
    assert!(quiet > QUIET_ROUNDS, "{quiet}");
    assert_eq!(busy, 5 + QUIET_ROUNDS, "{busy} rounds against {quiet}");
}

/// A slow path anywhere during the quiet rounds starts the count again, even one that leaves
/// no magazine behind and so changes nothing else the thread can see.
#[test]
fn a_slow_path_during_the_quiet_rounds_starts_the_count_again() {
    let rig = rig(12, TIGHT);
    one_slow_path(&rig);
    started(&rig);
    let keeper = SimCpu::new(1);
    let thread = rig.cache(&keeper);
    let key = thread.thread_key();
    let mut seen = Seen::default();
    let mut rounds = 0;
    let mut slow_path_at = None;
    loop {
        let (rest, ran) = one_call(&thread, &mut seen, key);
        rounds += u64::from(ran);
        if rest == Rest::Park {
            break;
        }
        if rounds == QUIET_ROUNDS - 1 && slow_path_at.is_none() {
            // A slow path on CPU 0, drained by now, that a later round drains again.
            let user = SimCpu::new(0);
            let program = rig.cache(&user);
            let object = program.alloc(tiny_class()).unwrap();
            program.free(tiny_class(), object);
            slow_path_at = Some(rounds);
        }
        assert!(rounds < 64, "did not park");
        rig.os().advance_clock(IDLE_TICKS / 2);
    }
    assert!(rounds > slow_path_at.unwrap() + QUIET_ROUNDS, "{rounds}");
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
    let key = started(&rig);
    let keeper = SimCpu::new(1);
    let thread = rig.cache(&keeper);
    let mut seen = Seen::default();
    rounds_until_parked(&rig, &thread, &mut seen, 1, 32);
    let word = &rig.source.thread;
    assert_eq!(word.load(), Snapshot(key | PARKED));
    assert!(cpu_0_is_empty(&rig), "parked with a loaded slab");
    assert_eq!(rig.os().wakes(), 0);
    swing(1);
    assert_eq!(
        rig.os().wakes(),
        1,
        "the resuming CPU's slow path did not wake the thread"
    );
    assert_eq!(word.load(), Snapshot(key | AWAKE));
    swing(1);
    assert_eq!(rig.os().wakes(), 1, "a slow path woke an awake thread");
}

/// Where drains are off, the thread never parks.
#[test]
fn the_thread_does_not_park_without_drains() {
    let rig = rig(12, TIGHT);
    rig.os().set_faults(Call::RegisterFence, Faults::ALWAYS);
    one_slow_path(&rig);
    let key = started(&rig);
    let keeper = SimCpu::new(1);
    let thread = rig.cache(&keeper);
    let mut seen = Seen::default();
    for _ in 0..4 * QUIET_ROUNDS {
        rig.os().advance_clock(IDLE_TICKS);
        let (rest, _) = one_call(&thread, &mut seen, key);
        assert_eq!(rest, Rest::Sleep, "parked without drains");
    }
}

/// A heap the thread has drained and found quiet for long enough, ready for its look before
/// parking: binds the rig, the thread's cache, its key and what it has seen.
macro_rules! ready_to_park {
    ($rig:ident, $thread:ident, $key:ident, $seen:ident) => {
        let $rig = rig(12, TIGHT);
        let keeper = SimCpu::new(1);
        let $thread = $rig.cache(&keeper);
        one_slow_path(&$rig);
        let $key = started(&$rig);
        let mut $seen = Seen::default();
        for _ in 0..QUIET_ROUNDS + 4 {
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
    ready_to_park!(rig, thread, key, seen);
    let activity = thread.first_look(&mut seen, false).unwrap();
    // The slow path, all of it before the store: it finds the word awake and wakes nobody.
    let user = SimCpu::new(0);
    let program = rig.cache(&user);
    let object = program.alloc(tiny_class()).unwrap();
    assert_eq!(rig.os().wakes(), 0);
    assert!(thread.mark_parked(key));
    assert_eq!(thread.second_look(key, activity), Rest::Sleep);
    assert_eq!(rig.source.thread.load(), Snapshot(key | AWAKE));
    program.free(tiny_class(), object);
}

/// A slow path whose load came after the store finds `PARKED` and wakes the thread; the thread's
/// second look backs out or finds the word already moved, and does not wait.
#[test]
fn a_slow_path_after_the_store_wakes_the_thread() {
    ready_to_park!(rig, thread, key, seen);
    let activity = thread.first_look(&mut seen, false).unwrap();
    assert!(thread.mark_parked(key));
    let user = SimCpu::new(0);
    let program = rig.cache(&user);
    let object = program.alloc(tiny_class()).unwrap();
    assert_eq!(rig.os().wakes(), 1);
    assert_eq!(rig.source.thread.load(), Snapshot(key | AWAKE));
    assert_eq!(thread.second_look(key, activity), Rest::Sleep);
    assert_eq!(rig.source.thread.load(), Snapshot(key | AWAKE));
    program.free(tiny_class(), object);
}

/// A level raised between the thread's store and its back-out is kept, and a parked word with
/// a level is still woken by the next slow path.
#[test]
fn a_level_raised_while_the_thread_parks_is_not_lost() {
    ready_to_park!(rig, thread, key, seen);
    let activity = thread.first_look(&mut seen, false).unwrap();
    assert!(thread.mark_parked(key));
    rig.source.thread.raise_level(true);
    assert_eq!(rig.source.thread.load(), Snapshot(key | PARKED));
    rig.source.thread.raise_level(false);
    assert_eq!(rig.source.thread.load(), Snapshot(key | PARKED | LEVEL));
    assert_eq!(thread.second_look(key, activity), Rest::Park);
    one_slow_path(&rig);
    assert_eq!(rig.os().wakes(), 1);
    assert_eq!(rig.source.thread.load(), Snapshot(key | AWAKE));
}

/// The look at the level: with no thread of the program's own it only clears the bit; with
/// one it wants the thread; it wakes a dormant thread; a slow path's wake answers `PARKED`
/// only, not `DORMANT`; and one claim creates the thread, a second finding it running.
#[test]
fn a_level_wants_or_wakes_the_thread_and_one_claim_creates_it() {
    let rig = rig(12, TIGHT);
    one_slow_path(&rig);
    let keeper = SimCpu::new(1);
    let key = rig.cache(&keeper).thread_key();
    let word = &rig.source.thread;
    word.raise_level(true);
    slow_path_end(&rig);
    assert_eq!(word.load(), Snapshot(key | NONE), "no thread of its own");
    rig.os().set_threads(true);
    word.raise_level(true);
    slow_path_end(&rig);
    assert_eq!(word.load(), Snapshot(key | WANTED));
    assert!(word.claim_start(rig.os(), key, DORMANT));
    assert!(!word.claim_start(rig.os(), key, DORMANT));
    slow_path_end(&rig);
    assert_eq!(rig.os().wakes(), 0, "a slow path woke a dormant thread");
    word.raise_level(true);
    slow_path_end(&rig);
    assert_eq!(word.load(), Snapshot(key | AWAKE));
    assert_eq!(rig.os().wakes(), 1);
    assert!(!word.claim_start(rig.os(), key, AWAKE), "created twice");
    // A running thread's level is cleared and nothing else changes.
    word.raise_level(false);
    slow_path_end(&rig);
    assert_eq!(word.load(), Snapshot(key | AWAKE));
    assert_eq!(rig.os().wakes(), 1);
}

/// A fork child reads its parent's word as another generation's: a parked word wakes nobody
/// and becomes the child's `NONE`; of two claims at once one creates the child's thread; and
/// the child of a threaded parent wants a thread only once a thread of its own crosses the
/// floor, not when its first thread does, though the C library's record still says the
/// parent's threads. A grandchild is a child too.
#[test]
fn a_fork_child_reads_its_parents_word_as_stale() {
    let rig = rig(12, TIGHT);
    one_slow_path(&rig);
    rig.os().set_threads(true);
    let keeper = SimCpu::new(1);
    let parent = rig.cache(&keeper).thread_key();
    let word = &rig.source.thread;
    word.set(parent | PARKED);
    // SAFETY: nothing else runs.
    unsafe { rig.os().wipe_for_fork() };
    let child = rig.cache(&keeper).thread_key();
    assert_ne!(child, parent);
    slow_path_end(&rig);
    assert_eq!(rig.os().wakes(), 0, "the child woke its parent's thread");
    assert_eq!(word.load(), Snapshot(child | NONE));
    // The parent's running thread does not stop the child creating its own, and two claims
    // in the child create one.
    word.set(parent | AWAKE);
    assert!(word.claim_start(rig.os(), child, AWAKE));
    assert!(!word.claim_start(rig.os(), child, AWAKE));
    // The parent's `OFF` does not hold in the child, nor its `WANTED`.
    word.set(parent | OFF);
    assert!(!word.claim(child, OFF, NONE));
    assert_eq!(word.load(), Snapshot(child | NONE));
    word.set(parent | WANTED);
    assert!(!word.claim(child, WANTED, AWAKE));
    assert_eq!(word.load(), Snapshot(child | NONE));
    // Over the floor, the child wants a thread only when a thread of its own crosses it.
    word.raise_level(true);
    slow_path_end(&rig);
    assert_eq!(
        word.load(),
        Snapshot(child | NONE),
        "the child's first thread"
    );
    rig.os().set_other_thread(true);
    word.raise_level(true);
    slow_path_end(&rig);
    assert_eq!(word.load(), Snapshot(child | WANTED));
    // The grandchild.
    // SAFETY: nothing else runs.
    unsafe { rig.os().wipe_for_fork() };
    let grandchild = rig.cache(&keeper).thread_key();
    assert_ne!(grandchild, child);
    // The child's `WANTED` holds back a crossing's request, which only a word that reads no
    // thread makes; the look once an interval asks whatever the word reads.
    word.raise_level(true);
    assert!(!word.load().level());
    word.raise_level(false);
    slow_path_end(&rig);
    assert_eq!(
        word.load(),
        Snapshot(grandchild | NONE),
        "the grandchild's first thread"
    );
    rig.os().set_other_thread(true);
    word.raise_level(true);
    slow_path_end(&rig);
    assert_eq!(word.load(), Snapshot(grandchild | WANTED));
}

/// The parent itself, not a fork child, takes the C library's record: its first thread's
/// crossing wants the thread once the program has created another.
#[test]
fn outside_a_fork_child_the_first_thread_may_want_the_thread() {
    let rig = rig(12, TIGHT);
    one_slow_path(&rig);
    let keeper = SimCpu::new(1);
    let key = rig.cache(&keeper).thread_key();
    rig.os().set_threads(true);
    rig.source.thread.raise_level(true);
    slow_path_end(&rig);
    assert_eq!(rig.source.thread.load(), Snapshot(key | WANTED));
}

/// A child that shares its parent's memory (`vfork`, `CLONE_VM`) shares its generation too: it
/// reads the parent's running thread as running, and creates no second one.
#[test]
fn a_child_that_shares_memory_is_not_a_new_process() {
    let rig = rig(12, TIGHT);
    one_slow_path(&rig);
    let key = started(&rig);
    let keeper = SimCpu::new(1);
    assert_eq!(rig.cache(&keeper).thread_key(), key);
    assert!(!rig.source.thread.claim_start(rig.os(), key, AWAKE));
}
