//! The middle tier's memory orders on real threads: what `docs/research/memory-ordering.md` says
//! the depot, its hints and the kernel's fence each rely on, checked where a store can be late.

use super::domain::{Domain, Domains};
use super::test_rig::{Rig, Small};
use super::{Pool, Tuning};
use crate::classes::SmallClass;
use crate::os::Pinned;
use crate::os::sim::{SimOs, SimTopology};
use crate::stress::{FULL, Gate, QUICK, Rounds, jitter, spread};
use crate::thread_word::{AWAKE, Process, ThreadWord};
use core::ptr::{self, NonNull};
use core::sync::atomic::AtomicU64;
use core::sync::atomic::Ordering::{Relaxed, SeqCst};
use std::time::Duration;

/// The class with the largest magazines: their first and last slots are lines apart.
fn wide_class() -> SmallClass {
    SmallClass::from_ordinal(0).unwrap()
}

/// A value for a magazine's slots: never null, as a slot's object is not.
fn value(n: usize) -> NonNull<u8> {
    NonNull::new(ptr::without_provenance_mut((n + 1) * 8)).unwrap()
}

fn number(slot: *mut u8) -> usize {
    slot.addr() / 8 - 1
}

/// Magazines go round a depot stack, each taker checking its first and last slots and adding one to
/// both. The push's release and the pop's acquire are all that carries a magazine's contents from
/// one thread to the next, so a taker that misses either write, or the record, has no
/// happens-before edge from the pusher. The count at the end is the number of takes, so a stale
/// pair that agreed is caught too.
fn a_depot_stack_hands_over_what_its_pusher_wrote(time: Duration) {
    let class = wide_class();
    let size = class.magazine();
    let capacity = size.capacity();
    let rig: Rig<SimOs<Small>, 2> = Rig::new(&[class], 0, Tuning::DEFAULT);
    {
        let area = rig.cache(Pinned(-1)).area();
        for _ in 0..2 {
            let magazine = area.create(size).expect("room for a magazine");
            magazine.set_object(1, value(0));
            magazine.set_object(capacity, value(0));
            magazine.set_level(capacity);
            rig.depot.full(Pool::Shared, class).push(area, magazine, 1);
        }
    }
    let rounds = Rounds::of(time, 200);
    let taken: usize = spread(2, |_| {
        let area = rig.cache(Pinned(-1)).area();
        let stack = rig.depot.full(Pool::Shared, class);
        let mut taken = 0;
        let mut round = 0;
        while rounds.go_on(round) {
            round += 1;
            let Some(magazine) = stack.pop(area, size) else {
                core::hint::spin_loop();
                continue;
            };
            assert_eq!(magazine.level(), capacity, "the record of the last owner");
            let last = number(magazine.object(capacity));
            let first = number(magazine.object(1));
            assert_eq!(first, last, "a popper did not see all its pusher wrote");
            magazine.set_object(1, value(first + 1));
            magazine.set_object(capacity, value(first + 1));
            magazine.set_level(capacity);
            stack.push(area, magazine, 1);
            taken += 1;
        }
        taken
    })
    .into_iter()
    .sum();
    let area = rig.cache(Pinned(-1)).area();
    let stack = rig.depot.full(Pool::Shared, class);
    let counted: usize = stack
        .magazines(area, size)
        .iter()
        .map(|magazine| number(magazine.object(1)))
        .sum();
    assert_eq!(counted, taken, "a popper took a stale magazine");
}

#[test]
fn a_depot_stack_hands_over_what_its_pusher_wrote_quickly() {
    a_depot_stack_hands_over_what_its_pusher_wrote(QUICK);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_depot_stack_hands_over_what_its_pusher_wrote_fully() {
    a_depot_stack_hands_over_what_its_pusher_wrote(FULL);
}

/// A push onto an empty stack sets the class's bit after its compare-and-swap, and a look that
/// finds the stack empty clears the bit and then reads the head again (`held.rs`): each writes one
/// word and then reads the other, so only sequential consistency rules out both reads missing.
/// A stack left holding a magazine with its bit clear is the failure.
fn a_push_and_a_look_leave_the_hint_set(time: Duration) {
    let class = wide_class();
    let size = class.magazine();
    let rig: Rig<SimOs<Small>, 2> = Rig::new(&[class], 0, Tuning::DEFAULT);
    let domain = Domain::ZERO;
    let held = rig.cache(Pinned(0)).area().create(size).unwrap();
    held.set_level(1);
    let address = held.base().addr().get();
    let stack = rig.depot.full(Pool::Domain(domain), class);
    let gate = Gate::new(2);
    let rounds = Rounds::of(time, 100);
    spread(2, |me| {
        let cache = rig.cache(Pinned(me as i32));
        let area = cache.area();
        let mut noise = 0x2545_f491_4f6c_dd1du64 + me as u64;
        let mut round = 0;
        loop {
            // Thread 0 holds the magazine; the stack is empty and its bit stale.
            if me == 0 {
                cache.hold(domain, class);
            }
            gate.pass();
            if !rounds.go_on(round) {
                return;
            }
            round += 1;
            jitter(&mut noise);
            if me == 0 {
                let magazine = area.magazine(address, size).unwrap();
                let pushed = stack.push_under(area, magazine, 1, u32::MAX);
                if pushed.is_ok_and(|pushed| pushed.onto_empty) {
                    cache.hold(domain, class);
                }
            } else {
                cache.unhold(domain, class);
            }
            gate.pass();
            if me == 0 {
                let bit = 1u64 << class.ordinal();
                assert_ne!(
                    rig.depot.held(domain).load(SeqCst) & bit,
                    0,
                    "a stack holds a magazine and its hint is clear"
                );
                stack.pop(area, size).expect("the magazine just pushed");
            }
            gate.pass();
        }
    });
}

#[test]
fn a_push_and_a_look_leave_the_hint_set_quickly() {
    a_push_and_a_look_leave_the_hint_set(QUICK);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_push_and_a_look_leave_the_hint_set_fully() {
    a_push_and_a_look_leave_the_hint_set(FULL);
}

/// Whoever makes work due and then reads the thread's word, against the thread's mark and then its
/// read of the work (`crate::thread_word`, "Sleeping"): the thread finds the work, or the waker
/// finds the mark and takes it.
fn a_sleeper_or_its_waker_sees_the_other(time: Duration) {
    let os: SimOs<Small> = SimOs::new();
    let word = ThreadWord::new();
    let work = AtomicU64::new(0);
    let key = 1u64 << crate::thread_word::KEY_SHIFT;
    let process = Process { key, forked: false };
    let gate = Gate::new(2);
    let rounds = Rounds::of(time, 100);
    spread(2, |me| {
        let mut noise = 0x9e37_79b9_7f4a_7c15u64 + me as u64;
        let mut round = 0;
        loop {
            if me == 0 {
                word.set(key | AWAKE);
                work.store(0, SeqCst);
            }
            gate.pass();
            if !rounds.go_on(round) {
                return;
            }
            round += 1;
            jitter(&mut noise);
            let found = if me == 0 {
                word.begin_sleep(key);
                work.load(SeqCst) != 0
            } else {
                work.fetch_add(1, SeqCst);
                let _ = word.rouse(&os, process);
                true
            };
            gate.pass();
            if me == 0 && !found {
                assert!(
                    !word.load().sleeping(),
                    "the thread sleeps with work due, and its waker left its mark"
                );
            }
            gate.pass();
        }
    });
}

#[test]
fn a_sleeper_or_its_waker_sees_the_other_quickly() {
    a_sleeper_or_its_waker_sees_the_other(QUICK);
}

#[test]
#[ignore = "slow: run with scripts/gate --all"]
fn a_sleeper_or_its_waker_sees_the_other_fully() {
    a_sleeper_or_its_waker_sees_the_other(FULL);
}

/// A reader that sees the domains' count published sees the table it counts.
fn the_domains_table_is_there_once_its_count_is(time: Duration) {
    let rounds = Rounds::of(time, 20);
    let mut round = 0;
    while rounds.go_on(round) {
        round += 1;
        let os: SimOs<Small> = SimOs::new();
        os.set_topology(SimTopology::Blocks(4));
        let domains = std::boxed::Box::new(Domains::new());
        let done = AtomicU64::new(0);
        spread(2, |me| {
            if me == 0 {
                domains.discover(&os);
                done.store(1, SeqCst);
            } else {
                loop {
                    let over = done.load(SeqCst) != 0;
                    if domains.count() > 1 {
                        // Under Miri a simulated machine has eight CPUs.
                        for cpu in 0..8 {
                            assert_eq!(domains.of(Some(cpu)).index(), cpu / 4);
                        }
                    }
                    if over {
                        return;
                    }
                }
            }
        });
    }
}

#[test]
fn the_domains_table_is_there_once_its_count_is_quickly() {
    the_domains_table_is_there_once_its_count_is(Duration::from_millis(100));
}

/// The kernel's fence itself, on the real kernel with no allocator in the way: the facts the drain
/// and the background thread's parking rest on (`docs/research/linux-rseq-contract.md`,
/// "membarrier"). The other thread runs plain loads and stores, as a routine does.
#[cfg(all(target_os = "linux", not(miri)))]
mod kernel_fence {
    use super::*;
    use core::sync::atomic::fence;

    #[cfg(target_arch = "x86_64")]
    const MEMBARRIER: i64 = 324;
    #[cfg(target_arch = "aarch64")]
    const MEMBARRIER: i64 = 283;
    const EXPEDITED_RSEQ: i64 = 1 << 7;
    const REGISTER_EXPEDITED_RSEQ: i64 = 1 << 8;
    const FLAG_CPU: i64 = 1;

    unsafe extern "C" {
        fn syscall(number: i64, ...) -> i64;
        fn sched_getcpu() -> i32;
    }

    /// Registers for the fence; whether the kernel takes it.
    fn register() -> bool {
        // SAFETY: membarrier takes integers and changes nothing the program relies on.
        unsafe { syscall(MEMBARRIER, REGISTER_EXPEDITED_RSEQ, 0i64, 0i64) == 0 }
    }

    /// The fence of one CPU, or of every CPU, as `LinuxOs` makes it.
    fn membarrier(cpu: Option<i32>) {
        let (flags, target) = match cpu {
            Some(cpu) => (FLAG_CPU, i64::from(cpu)),
            None => (0, 0),
        };
        // SAFETY: as in `register`.
        let done = unsafe { syscall(MEMBARRIER, EXPEDITED_RSEQ, flags, target) };
        assert_eq!(done, 0, "a fence the kernel refused");
    }

    use crate::stress::spread_still;

    /// Two words far enough apart that their stores can pass.
    #[repr(C, align(256))]
    struct Apart(AtomicU64);

    /// A store and then a load on both sides, one with only plain instructions and the other with
    /// a fence between them: the shape of the parking protocol, where a slow path stores into a
    /// slab and then loads the thread's word (`depot::thread`). Both loads cannot miss. With
    /// `fenced` false the fence is left out, and the outcome the fence rules out should appear:
    /// the count of such rounds is what this returns.
    fn store_then_load(time: Duration, aim: bool, fenced: bool) -> u64 {
        let (x, y) = (Apart(AtomicU64::new(0)), Apart(AtomicU64::new(0)));
        let target = core::sync::atomic::AtomicI32::new(-1);
        let gate = Gate::new(2);
        let rounds = Rounds::of(time, 0);
        let both_missed = core::sync::atomic::AtomicU64::new(0);
        let seen = (AtomicU64::new(0), AtomicU64::new(0));
        spread_still(2, |me| {
            if me == 0 {
                // SAFETY: a plain query.
                target.store(unsafe { sched_getcpu() }, SeqCst);
            }
            let mut noise = 0x2545_f491_4f6c_dd1du64 + me as u64;
            let mut round = 0;
            loop {
                gate.pass();
                if !rounds.go_on(round) {
                    return;
                }
                round += 1;
                x.0.store(0, Relaxed);
                y.0.store(0, Relaxed);
                gate.pass();
                jitter(&mut noise);
                if me == 0 {
                    x.0.store(1, Relaxed);
                    seen.0.store(y.0.load(Relaxed), Relaxed);
                } else {
                    y.0.swap(1, SeqCst);
                    if fenced {
                        membarrier(aim.then(|| target.load(SeqCst)));
                    }
                    seen.1.store(x.0.load(Relaxed), Relaxed);
                }
                gate.pass();
                if me == 0 && seen.0.load(Relaxed) == 0 && seen.1.load(Relaxed) == 0 {
                    both_missed.fetch_add(1, Relaxed);
                }
            }
        });
        both_missed.load(Relaxed)
    }

    /// A thread stores `a` and then `b` with plain instructions, over and over. Another reads `b`,
    /// fences that thread's CPU, and reads `a`: what the thread stored before the fence the caller
    /// sees, whichever order the stores reached memory in. Scenario B in the kernel's own comment.
    /// With `fenced` false the count of rounds that saw `b` ahead of `a` is the store reordering
    /// the fence hides.
    fn stores_seen_after_the_fence(time: Duration, fenced: bool) -> u64 {
        let (a, b) = (Apart(AtomicU64::new(0)), Apart(AtomicU64::new(0)));
        let target = core::sync::atomic::AtomicI32::new(-1);
        let rounds = Rounds::of(time, 0);
        let stop = AtomicU64::new(0);
        let results = spread_still(2, |me| {
            let mut ahead = 0;
            // The writer on the higher CPU: a performance core, on a machine that numbers its
            // efficiency cores first, and on the M1 those are the ones whose stores pass.
            if me == 1 {
                // SAFETY: a plain query.
                target.store(unsafe { sched_getcpu() }, SeqCst);
                let mut n = 0u64;
                while stop.load(Relaxed) == 0 {
                    n += 1;
                    a.0.store(n, Relaxed);
                    b.0.store(n, Relaxed);
                }
            } else {
                while target.load(SeqCst) < 0 {
                    core::hint::spin_loop();
                }
                let mut round = 0;
                while rounds.go_on(round) {
                    round += 1;
                    let high = b.0.load(Relaxed);
                    if fenced {
                        membarrier(Some(target.load(SeqCst)));
                    } else {
                        // So that only the order of the writer's stores can show.
                        fence(core::sync::atomic::Ordering::Acquire);
                    }
                    let low = a.0.load(Relaxed);
                    if low < high {
                        ahead += 1;
                    }
                }
                stop.store(1, SeqCst);
            }
            ahead
        });
        results.iter().sum()
    }

    #[test]
    fn the_fence_of_one_cpu_orders_a_store_and_a_load_against_plain_ones() {
        if register() {
            assert_eq!(store_then_load(QUICK, true, true), 0);
        }
    }

    #[test]
    fn the_fence_of_every_cpu_orders_a_store_and_a_load_against_plain_ones() {
        if register() {
            assert_eq!(store_then_load(QUICK, false, true), 0);
        }
    }

    #[test]
    fn what_a_cpu_stored_before_the_fence_the_caller_sees() {
        if register() {
            assert_eq!(stores_seen_after_the_fence(QUICK, true), 0);
        }
    }

    /// The tests above have teeth only if the hardware shows the outcome when the fence is left
    /// out. Printed, not asserted: a CPU that happens never to show it is no failure of ours.
    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn without_the_fence_the_hardware_shows_what_it_hides() {
        let missed = store_then_load(FULL / 4, true, false);
        let ahead = stores_seen_after_the_fence(FULL / 4, false);
        std::eprintln!("both loads missed: {missed}; a later store seen first: {ahead}");
        assert!(register());
        assert_eq!(store_then_load(FULL / 4, true, true), 0);
        assert_eq!(store_then_load(FULL / 4, false, true), 0);
        assert_eq!(stores_seen_after_the_fence(FULL / 4, true), 0);
    }
}
