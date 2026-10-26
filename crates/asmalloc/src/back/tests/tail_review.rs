//! Adversarial review of the huge-page tail (`back::tail`, branch `huge-tail-rule` at
//! `3c2265e`). Each test states what the rule promises and fails where the code does not keep
//! it; the review's report says which fail and why.

use super::super::layout::{Grow, Hints};
use super::super::policy::TAIL_HEADROOM_HUGE_PAGES;
use super::super::purge::Scope;
use super::super::tail::{self, Change, Tail};
use super::super::tree::Host;
use super::super::word::Word;
use super::*;
use crate::space::RegionId;

const C: usize = 64;

fn class() -> SmallClass {
    small(C)
}

fn page() -> usize {
    Standard::GEOMETRY.page_size()
}

fn huge() -> usize {
    Standard::GEOMETRY.huge_page_size()
}

fn charged(os: &Sim) -> usize {
    // SAFETY: no other thread runs, and the back end and the tests store only integers.
    unsafe { os.charged_bytes() }
}

/// As `tail::stranded`: two regions of the class, every slot painted, all but the first slot
/// of each huge page given back.
fn stranded<'a>(back: &Back<'a, Sim>) -> (Vec<*mut u8>, Vec<*mut u8>) {
    let region = back.space().plan().region_size();
    let slots = fill(back, class(), 2 * region / C);
    for &p in &slots {
        paint(p, C, 7);
    }
    let (kept, freed): (Vec<*mut u8>, Vec<*mut u8>) =
        slots.iter().partition(|p| (p.addr() % huge()) < C);
    for chunk in freed.chunks(64) {
        // SAFETY: the test holds these slots and gives them up.
        unsafe { back.launder(class(), chunk) };
    }
    (kept, freed)
}

fn region_of<'a>(back: &Back<'a, Sim>, at: *mut u8) -> RegionId {
    let plan = back.space().plan();
    let base = back.space().region(plan.region(0).unwrap()).addr().get();
    plan.region((at.addr() - base) >> plan.region_shift())
        .unwrap()
}

fn word_of<'a>(back: &Back<'a, Sim>, region: RegionId) -> Tail {
    Tail::unpack(back.state.tails[region.index()].load())
}

fn age_and_beat<'a>(back: &Back<'a, Sim>, intervals: usize) {
    forget_reuse(back);
    for _ in 0..intervals {
        back.space().os().advance_clock(DECAY_TICKS);
        beat(back);
    }
}

fn huge_from<'a>(back: &Back<'a, Sim>, region: RegionId) -> u32 {
    let host = back.host(class().class()).unwrap();
    Hints::unpack(host.hints(region).load()).huge_from.unwrap()
}

/// "A step that only counts ... splits nothing, returns nothing" (`purge`, "The tail"; "The
/// floor": a pass held to nothing purges nothing). A step that returns part of a huge page
/// ends with the region's cursor inside it (every split on a slow path ends the step there).
/// The next step that resumes from that cursor skips the test of `may_return`, which
/// `tail_step` makes only at the top of a huge page, so a counting step splits and returns the
/// rest of the huge page.
#[test]
#[cfg_attr(miri, ignore)]
fn a_counting_step_that_resumes_inside_a_huge_page_returns_nothing() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (kept, _) = stranded(&back);
    forget_reuse(&back);
    let region = region_of(&back, kept[kept.len() - 1]);
    let host = back.host(class().class()).unwrap();
    let hp = host.layout().huge_pages();
    // A returning step, as a heartbeat's: one split, then it stops.
    let (purged, _, done, _) = back.purge_region(
        class().class(),
        region,
        usize::MAX,
        page(),
        true,
        Scope::Idle,
        false,
        true,
    );
    assert!(purged != 0 && !done);
    assert_eq!(os.calls(Call::SplitHuge), 1);
    let cursor = Hints::unpack(host.hints(region).load()).cursor;
    assert!(
        !cursor.is_multiple_of(hp),
        "the cursor is at a huge page's top"
    );
    // A counting step from there, as a pass held under the floor takes.
    let (splits, purges) = (os.calls(Call::SplitHuge), os.calls(Call::Purge));
    let (purged, _, _, _) = back.purge_region(
        class().class(),
        region,
        usize::MAX,
        0,
        true,
        Scope::Idle,
        true,
        true,
    );
    assert_eq!(
        (
            purged,
            os.calls(Call::SplitHuge) - splits,
            os.calls(Call::Purge) - purges
        ),
        (0, 0, 0),
        "a counting step returned memory (bytes, splits, purges)"
    );
}

/// The same for a step that has met its limit and walks on to count (`counting` with the goal
/// spent): the walk of the small-page part purges nothing then (`Step::Unit(unit) if purged >=
/// limit`), and the tail must not either.
#[test]
#[cfg_attr(miri, ignore)]
fn a_step_past_its_limit_returns_no_more_than_a_run_over() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let (kept, _) = stranded(&back);
    forget_reuse(&back);
    let region = region_of(&back, kept[kept.len() - 1]);
    // The background thread's step (no bound on splits), with a goal of one page, counting.
    let (purged, _, _, _) = back.purge_region(
        class().class(),
        region,
        usize::MAX,
        page(),
        true,
        Scope::IdleAnyRun,
        true,
        false,
    );
    // It may finish the run it is on: one run of the goal's length, rounded to the shortest
    // run a pass takes.
    assert!(
        purged <= super::super::policy::PURGE_MIN_RUN_PAGES as usize * page(),
        "a step with a goal of one page returned {} pages",
        purged / page()
    );
}

/// "On a request's slow path a split counts as a whole step, so such a step makes one at
/// most." A split the kernel refuses is a system call all the same (`MADV_COLD` is refused
/// with `EINVAL` over a locked mapping, so in a program that called `mlockall` every one is),
/// and it is not counted: one heartbeat asks once for every run its budget reaches.
#[test]
#[cfg_attr(miri, ignore)]
fn a_heartbeat_asks_for_at_most_one_split_when_the_kernel_refuses() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (_, _) = stranded(&back);
    os.set_faults(Call::SplitHuge, Faults::ALWAYS);
    forget_reuse(&back);
    os.advance_clock(DECAY_TICKS);
    let mut most = 0;
    for _ in 0..BEATS {
        let before = os.calls(Call::SplitHuge);
        back.heartbeat();
        most = most.max(os.calls(Call::SplitHuge) - before);
    }
    assert!(most <= 1, "a heartbeat asked for {most} refused splits");
}

/// "Nothing waits or loops" and the thread sleeps in a quiet program. A region whose word
/// carries a claim's mark and an owner bit whose owner does not run (a thread a fork left
/// behind, or one stopped mid-change) is marked again by every look (`raise_marked`), and
/// `purge_due` is true while any region is marked, so the background thread never sleeps: its
/// steps do not count intervals, so in a program whose slow paths have gone quiet the owner
/// never goes stale either.
#[test]
#[cfg_attr(miri, ignore)]
fn the_thread_sleeps_though_an_owner_that_does_not_run_holds_a_marked_region() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (kept, _) = stranded(&back);
    age_and_beat(&back, 12);
    let region = region_of(&back, kept[kept.len() - 1]);
    let word = word_of(&back, region);
    assert_ne!(word.from, 0);
    // The thread's loop, until it would sleep.
    let settle = |back: &Back<'_, Sim>| {
        for _ in 0..BEATS {
            while back.thread_purge_steps(&|| back.now()).more {}
            if !back.purge_due(&|| back.now()) {
                return true;
            }
        }
        false
    };
    assert!(settle(&back), "the thread never sleeps on a quiet heap");
    // An owner took the bit in this interval and does not run; a claim had marked the region.
    let stuck = Tail {
        busy: true,
        want: true,
        stamp: back.epoch() as u32,
        ..word
    };
    back.state.tails[region.index()].store(stuck.pack());
    back.state.marks.store(1);
    assert!(
        settle(&back),
        "the thread spins: purge_due stays true while the owner does not run"
    );
    // Nor does time passing help, with no slow path to count the intervals.
    for _ in 0..8 {
        os.advance_clock(DECAY_TICKS);
        assert!(settle(&back), "still spinning intervals later");
    }
}

/// `tla/Tail.tla`, `Call`: a refused lowering keeps the noted huge page (`!.low = m.low`), so
/// that the advice comes off there once the kernel takes the call. The code drops it (`lower`
/// gives back `low: None` with the repair bit) when the lowering call itself is the one
/// refused, and nothing notes that huge page again, since its free pages have gone back.
#[test]
#[cfg_attr(miri, ignore)]
fn a_refused_lowering_leaves_the_noted_huge_page_in_the_word() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (kept, _) = stranded(&back);
    let region = region_of(&back, kept[kept.len() - 1]);
    let host = back.host(class().class()).unwrap();
    let from = huge_from(&back, region);
    let unit = from / host.layout().huge_pages() + 1;
    tail::note(&host, region, unit);
    os.set_faults(Call::AdviseHuge, Faults::ALWAYS);
    assert_eq!(
        tail::lower(&host, region, from, back.epoch()),
        Change::Refused
    );
    let word = word_of(&back, region);
    assert!(word.repair);
    assert_eq!(word.low, Some(unit), "{word:?}");
}

/// `tla/Tail.tla`, `Give`: a raise to the region's end keeps a huge page noted below the old
/// boundary (`low1` drops it only when `n.from # 0 /\ low0 >= n.from`). The code's raise
/// filters with `next.from != 0 && low < next.from`, which drops every note when the boundary
/// goes away.
#[test]
#[cfg_attr(miri, ignore)]
fn a_raise_to_the_end_keeps_a_note_below_the_old_boundary() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let (kept, _) = stranded(&back);
    let region = region_of(&back, kept[kept.len() - 1]);
    let host = back.host(class().class()).unwrap();
    let layout = *host.layout();
    let from = huge_from(&back, region);
    let first = from / layout.huge_pages();
    let units = layout.region_pages() / layout.huge_pages();
    assert!(first + 2 < units, "{first} of {units}");
    // The boundary at the last huge page; then the walk returns part of a lower one.
    tail::note(&host, region, units - 1);
    assert_eq!(tail::lower(&host, region, from, 1), Change::Done);
    tail::note(&host, region, first + 1);
    assert!(tail::mark(&host, region, layout.region_pages()));
    assert_eq!(
        tail::raise(
            &host,
            region,
            from,
            layout.region_pages(),
            layout.region_pages(),
            1
        ),
        Change::Done
    );
    let word = word_of(&back, region);
    assert_eq!(word.from, 0);
    assert_eq!(word.low, Some(first + 1), "{word:?}");
}

/// An owner is taken over only when its bit is stale. A thread whose reading of the interval
/// is one behind the owner's (it read the count, the count stepped, the owner took the bit:
/// `raise_marked` reads the count once for all its regions and makes system calls between)
/// computes an age of minus one, which the unsigned comparison takes for a very old bit, and
/// takes over an owner that took the bit a moment ago.
#[test]
#[cfg_attr(miri, ignore)]
fn a_fresh_owner_is_not_taken_over_by_a_thread_with_an_older_interval() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (kept, _) = stranded(&back);
    let region = region_of(&back, kept[kept.len() - 1]);
    let host = back.host(class().class()).unwrap();
    let from = huge_from(&back, region);
    let unit = from / host.layout().huge_pages() + 1;
    let owned = Tail {
        busy: true,
        stamp: 5,
        low: Some(unit),
        ..Tail::default()
    };
    back.state.tails[region.index()].store(owned.pack());
    let advised = os.calls(Call::AdviseHuge);
    let change = tail::lower(&host, region, from, 4);
    assert_eq!(
        (
            change,
            os.calls(Call::AdviseHuge) - advised,
            word_of(&back, region)
        ),
        (Change::None, 0, owned),
        "a thread at interval 4 took over an owner of interval 5"
    );
}

/// A change the kernel refuses is tried again by "the pass after", not at the rate of the
/// program's fills. With the kernel refusing advice (out of mappings, `vm.max_map_count`),
/// every fill that reaches above the boundary marks the region again, since the refused raise
/// cleared the mark, and every heartbeat then makes the refused calls again.
#[test]
#[cfg_attr(miri, ignore)]
fn a_refused_raise_is_not_tried_again_on_every_fill() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (_, _) = stranded(&back);
    age_and_beat(&back, 12);
    os.set_faults(Call::AdviseHuge, Faults::ALWAYS);
    let advised = os.calls(Call::AdviseHuge);
    let rounds = 40;
    let mut held = Vec::new();
    for _ in 0..rounds {
        held.extend(fill(&back, class(), 32));
        back.heartbeat();
    }
    let calls = os.calls(Call::AdviseHuge) - advised;
    // Two regions; a refused change in each, and no interval has passed.
    assert!(
        calls <= 4,
        "{calls} refused advice calls in {rounds} fills within one interval"
    );
}

/// "On a request's slow path" the work is bounded: a step, or a split. The raises a heartbeat
/// makes where there is no thread are not: one heartbeat changes the advice of every marked
/// region, each a call (three with a repair) that takes the address-space lock for writing.
#[test]
#[cfg_attr(miri, ignore)]
fn a_heartbeat_raises_a_bounded_number_of_regions() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let classes = [small(64), small(128), small(256), small(512)];
    let region = back.space().plan().region_size();
    let mut marked = 0;
    for class in classes {
        let size = class.size();
        let slots = fill(&back, class, 2 * region / size);
        for &p in &slots {
            paint(p, size, 7);
        }
        let freed: Vec<*mut u8> = slots
            .iter()
            .copied()
            .filter(|p| (p.addr() % huge()) >= size)
            .collect();
        for chunk in freed.chunks(64) {
            // SAFETY: the test holds these slots and gives them up.
            unsafe { back.launder(class, chunk) };
        }
    }
    age_and_beat(&back, 16);
    for class in classes {
        let _ = fill(&back, class, 2 * region / class.size());
    }
    for index in 0..back.walk_regions() {
        marked += usize::from(Tail::unpack(back.state.tails[index].load()).want);
    }
    assert!(marked >= 4, "{marked} regions marked");
    let advised = os.calls(Call::AdviseHuge);
    back.heartbeat();
    let calls = os.calls(Call::AdviseHuge) - advised;
    assert!(
        calls <= 1,
        "one heartbeat made {calls} advice calls for {marked} marked regions"
    );
}

/// What a raise puts back under the advice, khugepaged rebuilds if it holds one page, and the
/// headroom is by design huge pages that fills have not reached. Their returned pages are
/// resident and charged again, but their clean bits still say returned, so the retained count
/// does not hold them, the look calls them dense, and no pass returns them until the huge page
/// is wholly free. The test shows the memory coming back and staying.
#[test]
#[cfg_attr(miri, ignore)]
fn what_khugepaged_rebuilds_after_a_raise_is_returned_again() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (_, _) = stranded(&back);
    age_and_beat(&back, 12);
    let before = charged(os);
    // A few claims above the boundary, then the raise.
    let mut again = Vec::new();
    while back.state.marks.load() == 0 {
        let more = fill(&back, class(), 8);
        assert!(!more.is_empty());
        again.extend(more);
    }
    back.heartbeat();
    assert_eq!(back.state.marks.load(), 0);
    // SAFETY: as `charged`.
    let rebuilt = unsafe { os.khugepaged() };
    // The claims go back, and the passes run as before.
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class(), &again) };
    age_and_beat(&back, 12);
    let after = charged(os);
    assert!(
        after <= before + huge(),
        "khugepaged rebuilt {rebuilt} huge pages after the raise; charged {before} before, \
         {after} twelve intervals after the claims went back"
    );
}

/// The headroom of a raise: the advice goes back on up to the frontier and
/// `TAIL_HEADROOM_HUGE_PAGES` beyond, "never further". The standard shape's regions are four
/// huge pages, fewer than the headroom, so no other test reaches a raise that stops short of
/// the region's end. This one holds on the branch; it is here because nothing else fails when
/// the headroom, or the boundary a partial raise writes, is wrong.
#[test]
#[cfg_attr(miri, ignore)]
fn a_raise_stops_at_the_frontier_and_the_headroom() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let region_bytes = back.space().plan().region_size();
    let hp_bytes = Native::GEOMETRY.huge_page_size();
    let slots = fill(&back, class(), 4 * region_bytes / C);
    assert!(!slots.is_empty());
    let host = back.host(class().class()).unwrap();
    let layout = *host.layout();
    let hp = layout.huge_pages();
    let units = layout.region_pages() / hp;
    let plan = back.space().plan();
    let carved = |region: RegionId| {
        layout.carved_pages(Grow::unpack(host.grow(region).load()).carved)
    };
    let region = (0..back.walk_regions())
        .filter_map(|index| plan.region(index))
        .filter(|&region| back.state.table.get(region) == RegionTag::Class(class().class()))
        .max_by_key(|&region| carved(region))
        .unwrap();
    let top = carved(region);
    let from = Hints::unpack(host.hints(region).load()).huge_from.unwrap();
    let first = from / hp;
    assert!(
        first + 2 + TAIL_HEADROOM_HUGE_PAGES < units.min(top.div_ceil(hp)),
        "the region is too short for a partial raise: {first} of {units}, carved to {top}"
    );
    tail::note(&host, region, first + 1);
    assert_eq!(tail::lower(&host, region, from, 1), Change::Done);
    assert!(tail::mark(&host, region, (first + 1) * hp + 1));
    let frontier = (first + 1) * hp + 1;
    assert_eq!(
        tail::raise(&host, region, from, frontier, top, 1),
        Change::Done
    );
    let word = Tail::unpack(host.tail(region).load());
    let to = first + 2 + TAIL_HEADROOM_HUGE_PAGES;
    assert_eq!(word.from, to, "{word:?}");
    let region_base = back.space().region(region);
    for unit in first..units {
        let at = region_base.map_addr(|a| a.saturating_add(unit as usize * hp_bytes));
        let want = if unit >= to {
            HugePages::Avoid
        } else {
            HugePages::Prefer
        };
        assert_eq!(os.huge_pages(at), Some(want), "unit {unit}");
    }
}

/// The kernel's part, on the host itself: nothing else runs `LinuxOs::split_huge`, so a wrong
/// advice number there (`MADV_FREE` or `MADV_DONTNEED` would destroy a run that a fill may
/// claim between the split and the purge's own claim) passes every other test.
mod kernel {
    use crate::os::linux::LinuxOs;
    use crate::os::{HugePages, Os};
    use std::string::String;

    const PAGE: usize = 4096;
    const HUGE: usize = 2 << 20;

    /// `AnonHugePages` of the mapping that holds `addr`, in KiB.
    fn anon_huge_kb(addr: usize) -> usize {
        let maps = std::fs::read_to_string("/proc/self/smaps").unwrap();
        let mut inside = false;
        for line in maps.lines() {
            let first: String = line.chars().take_while(|c| !c.is_whitespace()).collect();
            if let Some((start, end)) = first.split_once('-')
                && let (Ok(start), Ok(end)) = (
                    usize::from_str_radix(start, 16),
                    usize::from_str_radix(end, 16),
                )
            {
                inside = (start..end).contains(&addr);
            } else if inside && let Some(rest) = line.strip_prefix("AnonHugePages:") {
                return rest.trim().trim_end_matches("kB").trim().parse().unwrap();
            }
        }
        panic!("no mapping holds {addr:#x}");
    }

    /// A split of part of a huge page keeps every byte, and where the kernel gave a huge page
    /// it is no longer mapped as one afterwards; the purge of the run then reads as zero and
    /// leaves the rest.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn split_huge_keeps_contents_and_splits_the_huge_page() {
        let os = LinuxOs;
        let base = os.reserve(2 * HUGE, HUGE).unwrap();
        let p = base.as_ptr();
        let run = base.map_addr(|a| a.saturating_add(PAGE));
        // SAFETY: each range is whole pages of the reservation, and nothing else uses it.
        unsafe {
            os.commit(base, 2 * HUGE).unwrap();
            os.advise_huge(base, 2 * HUGE, HugePages::Prefer).unwrap();
            for i in 0..HUGE / PAGE {
                p.add(i * PAGE + 7).write(i as u8 | 1);
            }
            let before = anon_huge_kb(base.addr().get());
            os.split_huge(run, 4 * PAGE).unwrap();
            let after = anon_huge_kb(base.addr().get());
            std::eprintln!("AnonHugePages {before} kB before the split, {after} after");
            for i in 0..HUGE / PAGE {
                assert_eq!(p.add(i * PAGE + 7).read(), i as u8 | 1, "page {i}");
            }
            if before >= HUGE / 1024 {
                assert!(after < before, "{before} kB huge before, {after} after");
            }
            os.purge(run, 4 * PAGE).unwrap();
            for i in 0..HUGE / PAGE {
                let want = if (1..5).contains(&i) { 0 } else { i as u8 | 1 };
                assert_eq!(p.add(i * PAGE + 7).read(), want, "page {i}");
            }
            os.unmap(base, 2 * HUGE).unwrap();
        }
    }

    unsafe extern "C" {
        fn mlock(addr: *const core::ffi::c_void, len: usize) -> i32;
    }

    /// The kernel refuses `MADV_COLD` over a locked mapping (`can_madv_lru_vma`), so in a
    /// program that locks its memory every split is refused: the case
    /// `a_heartbeat_asks_for_at_most_one_split_when_the_kernel_refuses` is about.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_locked_mapping_refuses_the_split() {
        let os = LinuxOs;
        let base = os.reserve(2 * HUGE, HUGE).unwrap();
        let run = base.map_addr(|a| a.saturating_add(PAGE));
        // SAFETY: each range is whole pages of the reservation, and nothing else uses it.
        unsafe {
            os.commit(base, 2 * HUGE).unwrap();
            base.as_ptr().write(1);
            if mlock(base.as_ptr().cast(), HUGE) != 0 {
                // No allowance to lock memory here: nothing to show.
                std::eprintln!("mlock refused: nothing shown");
                os.unmap(base, 2 * HUGE).unwrap();
                return;
            }
            let split = os.split_huge(run, 4 * PAGE);
            std::eprintln!("split of a locked mapping: {split:?}");
            assert!(split.is_err());
            os.unmap(base, 2 * HUGE).unwrap();
        }
    }
}
