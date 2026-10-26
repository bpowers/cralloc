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
    // The frontier aged: the advice may come off above it only (fix of finding 7).
    forget_reuse(&back);
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
    // The frontier aged: the advice may come off above it only (fix of finding 7).
    forget_reuse(&back);
    let epoch = back.epoch();
    let region = region_of(&back, kept[kept.len() - 1]);
    let host = back.host(class().class()).unwrap();
    let layout = *host.layout();
    let from = huge_from(&back, region);
    let first = from / layout.huge_pages();
    let units = layout.region_pages() / layout.huge_pages();
    assert!(first + 2 < units, "{first} of {units}");
    // The boundary at the last huge page; then the walk returns part of a lower one.
    tail::note(&host, region, units - 1);
    assert_eq!(tail::lower(&host, region, from, epoch), Change::Done);
    tail::note(&host, region, first + 1);
    assert!(tail::mark(&host, region, layout.region_pages()));
    assert_eq!(
        tail::raise(
            &host,
            region,
            from,
            layout.region_pages(),
            layout.region_pages(),
            epoch
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
    // A request's heartbeat raises one region a step (finding 3): as many as it takes.
    for _ in 0..8 {
        if back.state.marks.load() == 0 {
            break;
        }
        back.heartbeat();
    }
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
    let carved =
        |region: RegionId| layout.carved_pages(Grow::unpack(host.grow(region).load()).carved);
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
    // The frontier aged: the advice may come off above it only (fix of finding 7).
    tick(&back, 2);
    let epoch = back.epoch();
    tail::note(&host, region, first + 1);
    assert_eq!(tail::lower(&host, region, from, epoch), Change::Done);
    assert!(tail::mark(&host, region, (first + 1) * hp + 1));
    let frontier = (first + 1) * hp + 1;
    assert_eq!(
        tail::raise(&host, region, from, frontier, top, epoch),
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

/// The pages `tail::look` samples in `unit` at `salt`: its arithmetic, restated.
fn sampled(unit: super::super::layout::Unit, salt: u32) -> Vec<u32> {
    let pages = unit.end - unit.start;
    let samples = super::super::policy::TAIL_SAMPLE_PAGES.min(pages);
    let share = pages / samples;
    (0..samples)
        .map(|i| {
            let mix = (salt ^ i.wrapping_mul(0x9E37_79B9)).wrapping_mul(0x85EB_CA6B) >> 16;
            unit.start + i * share + mix % share
        })
        .collect()
}

/// "Each lap samples other pages, so a huge page misjudged once is judged again." The salt is
/// the count of intervals in which a heartbeat ran, which stands still through a silence: the
/// time when the background thread's rounds return a quiet program's memory. A huge page
/// above the frontier with half its pages free, none of them where this interval's samples
/// fall, is called dense by every lap of the silence and keeps all of it.
#[test]
#[cfg_attr(miri, ignore)]
fn a_huge_page_misjudged_once_is_judged_again_in_a_silence() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let region_bytes = back.space().plan().region_size();
    let slots = fill(&back, class(), 2 * region_bytes / C);
    for &p in &slots {
        paint(p, C, 7);
    }
    forget_reuse(&back);
    let region = region_of(&back, slots[0]);
    let host = back.host(class().class()).unwrap();
    let layout = *host.layout();
    let hp = layout.huge_pages();
    let top = layout.carved_pages(Grow::unpack(host.grow(region).load()).carved);
    let start = (top & !(hp - 1)) - hp;
    assert!(start >= huge_from(&back, region) && start > 0);
    let unit = super::super::layout::Unit {
        start,
        end: start + hp,
    };
    let salt = back.epoch() as u32;
    let seen = sampled(unit, salt);
    let base = back.space().region(region).addr().get();
    let freed: Vec<*mut u8> = slots
        .iter()
        .copied()
        .filter(|p| {
            let page = ((p.addr() - base) / page()) as u32;
            (unit.start..unit.end).contains(&page) && !seen.contains(&page)
        })
        .collect();
    assert!(freed.len() * C >= (hp as usize - seen.len()) * page() / 2);
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class(), &freed) };
    let look = tail::look(&host, region, unit, salt);
    assert!(
        !look.sparse && look.free == 0,
        "the restated samples differ: {look:?}"
    );
    assert!(
        (1..8).any(|later| tail::look(&host, region, unit, salt + later).sparse),
        "no later salt finds it either"
    );
    // A silence: the thread's rounds, an interval apart, and no slow path.
    let before = charged(os);
    for _ in 0..24 {
        os.advance_clock(DECAY_TICKS);
        while back.thread_purge_steps(&|| back.now()).more {}
    }
    assert_eq!(back.epoch() as u32, salt, "the silence counted an interval");
    let after = charged(os);
    assert!(
        before.saturating_sub(after) >= freed.len() * C / 2,
        "{} bytes free in whole pages of one huge page above the frontier; charged {before} \
         -> {after} after 24 intervals of silence",
        freed.len() * C
    );
}

/// A boundary of zero is no boundary, so `tail::note` passes the region's first huge page:
/// "a small class's slots start a few pages in (its origin), so that huge page is never
/// mapped whole". That rests on every class that gets huge pages having an origin past the
/// region's first page, in every geometry; nothing else checks it.
#[test]
fn every_class_with_huge_pages_starts_past_the_first_page() {
    // The simulator's standard shape does not keep it (its 16-byte class starts in the first
    // page), so no simulated test can show what happens there on the design's geometry.
    for geometry in [crate::space::Geometry::DESIGN] {
        let plan = geometry.largest_plan();
        for class in crate::classes::SmallClass::all() {
            if !super::super::policy::wants_huge(class.size(), geometry.page_size()) {
                continue;
            }
            assert!(
                plan.slot_origin(class.class()) >= geometry.page_size(),
                "class of {} bytes has its origin in the first page of a {} KiB region",
                class.size(),
                plan.region_size() >> 10
            );
        }
    }
}

/// A kernel that refuses every change of advice, from the region's first (no huge pages
/// built in, or out of mappings throughout): the rule still returns the free pages, nothing
/// is lost, the word asks for a repair and never names a boundary, no claim marks a region,
/// and the refused calls are at most the reconcile's, once a region a pass. This holds on
/// the branch.
#[test]
#[cfg_attr(miri, ignore)]
fn a_kernel_that_refuses_all_advice_costs_a_call_a_region_a_pass() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    os.set_faults(Call::AdviseHuge, Faults::ALWAYS);
    let (kept, freed) = stranded(&back);
    let before = charged(os);
    let laps = |back: &Back<'_, Sim>| {
        back.state
            .walk
            .laps
            .load(core::sync::atomic::Ordering::Relaxed) as u64
    };
    let (advised, lap) = (os.calls(Call::AdviseHuge), laps(&back));
    age_and_beat(&back, 12);
    assert!(before - charged(os) > freed.len() * C / 2);
    for &p in &kept {
        assert!(painted(p, C, 7));
    }
    let mut repairs = 0;
    for index in 0..back.walk_regions() {
        let word = Tail::unpack(back.state.tails[index].load());
        assert!(word.from == 0 && !word.busy && !word.want, "{word:?}");
        repairs += u64::from(word.repair);
    }
    assert!(repairs > 0);
    assert_eq!(back.state.marks.load(), 0);
    let calls = os.calls(Call::AdviseHuge) - advised;
    let laps = laps(&back) - lap;
    // The lowering's own call, then the reconcile's one (the boundary is none) each pass, and
    // a pass may end partway through a region and try once more.
    assert!(
        calls <= 2 * repairs * (laps + 1),
        "{calls} refused advice calls for {repairs} regions in {laps} laps"
    );
    // Claims change no advice and mark nothing while there is no boundary.
    let advised = os.calls(Call::AdviseHuge);
    let again = fill(&back, class(), 4096);
    assert!(!again.is_empty());
    back.heartbeat();
    assert_eq!(os.calls(Call::AdviseHuge), advised);
    assert_eq!(back.state.marks.load(), 0);
}

/// `tla/Tail.tla`'s properties on the code itself, under the interleaving explorer
/// (`back::explore`): `tail`'s functions unchanged, every operation on the tail word a step,
/// over a kernel that records the advice of each huge page. The branch's own tests of the
/// protocol are one randomized race and the model, and nothing ties the model to the code:
/// the mutants of the take-over and of the give-back's merges all pass them.
mod protocol {
    use super::super::super::explore::{Config, Outcome, TWord, explore};
    use super::super::super::layout::Layout;
    use super::super::super::tail::{self, Tail};
    use super::super::super::testhost::TestHost;
    use super::super::super::tree::Host;
    use super::super::super::word::Word;
    use crate::classes::Class;
    use crate::os::HugePages;
    use crate::os::sim::{Shape, Standard};
    use crate::space::RegionId;
    use std::sync::Mutex;
    use std::vec::Vec;

    /// A test host whose kernel remembers the advice of each huge page of region 0.
    struct Kernel {
        host: TestHost<TWord>,
        /// Per huge page: advised for huge pages.
        on: Mutex<Vec<bool>>,
    }

    impl Kernel {
        fn new() -> Kernel {
            let plan = Standard::GEOMETRY.largest_plan();
            let host = TestHost::<TWord>::new(plan, Class::of_size(64).unwrap(), 1);
            let units = host.layout().region_pages() / host.layout().huge_pages();
            Kernel {
                host,
                on: Mutex::new(std::vec![true; units as usize]),
            }
        }

        fn region(&self) -> RegionId {
            self.host.region_at(0)
        }

        /// The first page of the huge-page part: the region's second huge page.
        fn huge(&self) -> u32 {
            self.host.layout().huge_pages()
        }

        fn units(&self) -> u32 {
            self.host.layout().region_pages() / self.host.layout().huge_pages()
        }

        fn word(&self) -> Tail {
            Tail::unpack(self.host.tail(self.region()).load())
        }

        /// `Agree`'s right-hand side: the kernel's advice over the huge-page part is the
        /// word's.
        fn agrees(&self, tail: Tail) -> bool {
            let on = self.on.lock().unwrap();
            (1..self.units()).all(|unit| on[unit as usize] == (tail.from == 0 || unit < tail.from))
        }

        /// `OffOnlyReturned`: advice is off only at or above `lowest`, the lowest huge page
        /// any walker noted.
        fn off_only_from(&self, lowest: u32) -> bool {
            let on = self.on.lock().unwrap();
            (0..lowest).all(|unit| on[unit as usize])
        }
    }

    impl Host for Kernel {
        type W = TWord;

        fn layout(&self) -> &Layout {
            self.host.layout()
        }
        fn region(&self, index: usize) -> Option<RegionId> {
            self.host.region(index)
        }
        fn meta(&self, region: RegionId, word: u32) -> &TWord {
            self.host.meta(region, word)
        }
        fn tree_words(&self, region: RegionId, level: u8, count: u32) -> &[TWord] {
            self.host.tree_words(region, level, count)
        }
        fn grow(&self, region: RegionId) -> &TWord {
            self.host.grow(region)
        }
        fn hints(&self, region: RegionId) -> &TWord {
            self.host.hints(region)
        }
        fn frontier(&self, region: RegionId) -> &TWord {
            self.host.frontier(region)
        }
        fn tail(&self, region: RegionId) -> &TWord {
            self.host.tail(region)
        }
        fn tail_marks(&self) -> &TWord {
            self.host.tail_marks()
        }
        fn class_word(&self, index: u32) -> &TWord {
            self.host.class_word(index)
        }
        fn class_top(&self) -> &TWord {
            self.host.class_top()
        }
        fn commit(&self, region: RegionId, offset: usize, len: usize) -> bool {
            self.host.commit(region, offset, len)
        }
        fn commit_meta(&self, region: RegionId, offset: usize, len: usize) -> bool {
            self.host.commit_meta(region, offset, len)
        }
        fn advise(&self, _: RegionId, offset: usize, len: usize, advice: HugePages) -> bool {
            let unit = self.layout().huge_pages() as usize * self.layout().page_size();
            assert!(offset.is_multiple_of(unit) && len.is_multiple_of(unit) && len > 0);
            let mut on = self.on.lock().unwrap();
            for at in offset / unit..(offset + len) / unit {
                on[at] = advice == HugePages::Prefer;
            }
            true
        }
        fn split(&self, region: RegionId, offset: usize, len: usize) -> bool {
            self.host.split(region, offset, len)
        }
        unsafe fn purge(&self, region: RegionId, offset: usize, len: usize) -> bool {
            // SAFETY: the caller's promise, passed on; no scenario here purges.
            unsafe { self.host.purge(region, offset, len) }
        }
        fn abort(&self, message: &str) -> ! {
            self.host.abort(message)
        }
    }

    /// After a schedule: `Agree` as the model states it, then the repair that the next change
    /// makes (in a fork's child, after the child's distrust of every word), and `Agree` again
    /// with nothing left to repair.
    fn check_agree(kernel: &Kernel, outcome: &Outcome, lowest: u32) {
        let region = kernel.region();
        let word = kernel.word();
        assert!(
            kernel.off_only_from(lowest),
            "advice off below {lowest}: {word:?}"
        );
        if outcome.killed.is_empty() {
            assert!(!word.busy, "an owner bit left set: {word:?}");
            if !word.repair {
                assert!(
                    kernel.agrees(word),
                    "Agree: {word:?} against {:?}",
                    kernel.on.lock().unwrap()
                );
            }
        } else if kernel.host.tail(region).load() != 0 {
            // `Back::distrust_tails`, in the child.
            kernel.host.tail(region).fetch_or(tail::REPAIR);
        }
        // Many intervals later, so that an owner the fork cut off is taken over.
        let _ = tail::lower(kernel, region, kernel.huge(), 1000);
        let word = kernel.word();
        if !outcome.killed.is_empty() && word.busy {
            // Nothing asked for a change (no note, and the word was zero but for the owner):
            // the bit waits for the next note.
            return;
        }
        assert!(!word.busy && !word.repair, "{word:?}");
        assert!(
            kernel.agrees(word),
            "Agree after the repair: {word:?} against {:?}",
            kernel.on.lock().unwrap()
        );
    }

    /// `Tail.cfg`: a lowering, a second lowering two intervals later that takes the first
    /// over if it is still under way, and a claim's mark with its raise. A boundary at the last
    /// huge page stands before they start.
    fn lowerings_and_a_raise(config: Config) -> usize {
        explore(
            config,
            3,
            || {
                let kernel = Kernel::new();
                let last = kernel.units() - 1;
                tail::note(&kernel, kernel.region(), last);
                let _ = tail::lower(&kernel, kernel.region(), kernel.huge(), 1);
                assert_eq!(kernel.word().from, last);
                kernel
            },
            |kernel, me| {
                let region = kernel.region();
                let layout = *kernel.layout();
                match me {
                    0 => {
                        let _ = tail::lower(kernel, region, kernel.huge(), 3);
                    }
                    1 => {
                        tail::note(kernel, region, 2);
                        let _ = tail::lower(kernel, region, kernel.huge(), 1);
                    }
                    _ => {
                        let _ = tail::mark(kernel, region, layout.region_pages());
                        let _ =
                            tail::raise(kernel, region, kernel.huge(), 0, layout.region_pages(), 3);
                    }
                }
            },
            |kernel, outcome| check_agree(kernel, outcome, 2),
        )
        .schedules
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn lowerings_and_a_raise_agree_quickly() {
        let schedules = lowerings_and_a_raise(Config {
            preemptions: Some(2),
            ..Config::ALL
        });
        assert!(schedules > 100, "{schedules} schedules");
    }

    #[test]
    #[ignore = "slow: run with scripts/gate --all"]
    fn lowerings_and_a_raise_agree_fully() {
        lowerings_and_a_raise(Config {
            preemptions: Some(4),
            ..Config::ALL
        });
    }

    /// The same with a fork that leaves a thread behind at any step (`ForkRepair`).
    #[test]
    #[cfg_attr(miri, ignore)]
    fn lowerings_and_a_raise_agree_after_a_fork_quickly() {
        lowerings_and_a_raise(Config {
            preemptions: Some(2),
            crash: true,
            ..Config::ALL
        });
    }

    /// A note made while another thread owns the word is kept by that owner's give-back
    /// (`Give`'s `low0`): two walkers note and lower, and once both are done the next lowering
    /// brings the boundary to the lowest huge page noted, in every schedule.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_note_made_during_a_lowering_is_lowered_next() {
        explore(
            Config::ALL,
            2,
            Kernel::new,
            |kernel, me| {
                let region = kernel.region();
                tail::note(kernel, region, if me == 0 { 2 } else { 1 });
                let _ = tail::lower(kernel, region, kernel.huge(), 1);
            },
            |kernel, outcome| {
                assert!(outcome.killed.is_empty());
                let _ = tail::lower(kernel, kernel.region(), kernel.huge(), 1);
                let word = kernel.word();
                assert_eq!((word.from, word.low), (1, None), "{word:?}");
                assert!(kernel.agrees(word));
            },
        );
    }

    /// A mark set while another thread owns the word is kept by that owner's give-back
    /// (`Give`'s `want`): the claim was told that it marked the region, so the mark is there
    /// when the lowering is done.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_mark_set_during_a_lowering_is_kept() {
        explore(
            Config::ALL,
            2,
            || {
                let kernel = Kernel::new();
                let last = kernel.units() - 1;
                tail::note(&kernel, kernel.region(), last);
                let _ = tail::lower(&kernel, kernel.region(), kernel.huge(), 1);
                (kernel, Mutex::new(false))
            },
            |(kernel, marked), me| {
                let region = kernel.region();
                if me == 0 {
                    tail::note(kernel, region, 2);
                    let _ = tail::lower(kernel, region, kernel.huge(), 1);
                } else {
                    let pages = kernel.layout().region_pages();
                    *marked.lock().unwrap() = tail::mark(kernel, region, pages);
                }
            },
            |(kernel, marked), _| {
                let word = kernel.word();
                assert!(
                    *marked.lock().unwrap(),
                    "the claim above the boundary did not mark"
                );
                assert!(word.want, "the mark was lost: {word:?}");
            },
        );
    }

    /// Second round, on the fixes (`d8c8fad`). `MarkVisible`: once nobody is at work, a word
    /// that carries a mark has the flag set. The model's take is one step; the code's raise
    /// loads the word and then takes the bit with a compare-and-swap, which a walker's note in
    /// between makes fail. The look has cleared the flag, no owner exists to set it again, and
    /// the mark stays where no look finds it until some later lowering of the region gives
    /// the bit back.
    #[test]
    #[cfg_attr(miri, ignore)]
    fn a_mark_is_visible_after_a_look_that_lost_its_take_to_a_note() {
        explore(
            Config::ALL,
            2,
            || {
                let kernel = Kernel::new();
                let region = kernel.region();
                let last = kernel.units() - 1;
                tail::note(&kernel, region, last);
                let _ = tail::lower(&kernel, region, kernel.huge(), 1);
                assert!(tail::mark(&kernel, region, kernel.layout().region_pages()));
                kernel.tail_marks().fetch_or(1);
                kernel
            },
            |kernel, me| {
                let region = kernel.region();
                if me == 0 {
                    // `Back::raise_marked`: clear the flag, then raise the marked region.
                    if kernel.tail_marks().compare_exchange(1, 0).is_ok() {
                        let pages = kernel.layout().region_pages();
                        let _ = tail::raise(kernel, region, kernel.huge(), 0, pages, 1);
                    }
                } else {
                    tail::note(kernel, region, 2);
                }
            },
            |kernel, _| {
                let word = kernel.word();
                assert!(
                    !word.want || kernel.tail_marks().load() != 0,
                    "a mark with the flag clear and nobody at work: {word:?}"
                );
            },
        );
    }

    /// The lowering "never below the frontier" drops a note the frontier has passed. The
    /// word keeps only the lowest huge page noted, so the drop also forgets every huge page
    /// above the frontier that the walk returned part of since the last lowering: their
    /// advice stays on. Lowering at the frontier instead keeps `OffOnlyReturned` (the
    /// frontier is above the noted huge page) and takes the advice off where it should.
    #[test]
    fn a_note_the_frontier_passed_still_lowers_what_lies_above_the_frontier() {
        let kernel = Kernel::new();
        let region = kernel.region();
        let layout = *kernel.layout();
        let hp = layout.huge_pages();
        // The walk returned part of huge pages 1 and 3; the word holds the lowest.
        tail::note(&kernel, region, 3);
        tail::note(&kernel, region, 1);
        // Then a fill reached into huge page 2.
        let format = layout.frontier_format();
        let reached = format.record(0, 1, hp + 1).unwrap();
        kernel.frontier(region).store(reached);
        let _ = tail::lower(&kernel, region, kernel.huge(), 1);
        let word = kernel.word();
        assert!(
            word.from != 0 && word.from <= 3,
            "huge page 3, returned in part and above the frontier, keeps its advice: {word:?}"
        );
    }
}

/// Second round. The cut (`TAIL_SPLIT_SHARE`) protects a huge page's mapping, but it is
/// applied to huge pages that are already split and whose advice is off: once a pass has
/// returned the long runs of one, what is left is under a quarter, and the pass that takes
/// runs of any length never takes it. Before the cut those pages went back at that pass.
#[test]
#[cfg_attr(miri, ignore)]
fn the_short_runs_of_a_split_huge_page_go_back_at_the_pass_for_short_runs() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let region_bytes = back.space().plan().region_size();
    let slots = fill(&back, class(), 2 * region_bytes / C);
    for &p in &slots {
        paint(p, C, 7);
    }
    let region = region_of(&back, slots[0]);
    let host = back.host(class().class()).unwrap();
    let layout = *host.layout();
    let hp = layout.huge_pages();
    assert_eq!(hp, 16);
    let top = layout.carved_pages(Grow::unpack(host.grow(region).load()).carved);
    let start = (top & !(hp - 1)) - hp;
    assert!(start >= huge_from(&back, region) && start > 0);
    let base = back.space().region(region).addr().get();
    // A run of four pages and two pages on their own: six of sixteen.
    let free_pages = [4, 5, 6, 7, 10, 12].map(|p| start + p);
    let freed: Vec<*mut u8> = slots
        .iter()
        .copied()
        .filter(|p| free_pages.contains(&(((p.addr() - base) / page()) as u32)))
        .collect();
    assert_eq!(freed.len() * C, 6 * page());
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class(), &freed) };
    let before = charged(os);
    age_and_beat(&back, 48);
    assert!(
        os.calls(Call::SplitHuge) > 0,
        "the huge page was never split"
    );
    let returned = before.saturating_sub(charged(os));
    assert!(
        returned >= 6 * page(),
        "{} of 6 free pages of a split huge page went back in 48 intervals",
        returned / page()
    );
}
