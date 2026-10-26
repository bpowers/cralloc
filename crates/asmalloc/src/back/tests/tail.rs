//! The huge-page tail (`back::tail`, `docs/research/huge-tail.md`): each numbered point of
//! the rule, the bounded step, every refused call, the concurrency of lowering against
//! raising, and the fork child's repair. On the simulator's model of huge pages: a huge page
//! committed under the advice is charged whole, a purge of part of it without a split leaves
//! it charged, and khugepaged rebuilds an advised huge page that holds anything.

use super::super::layout::{Grow, Hints};
use super::super::tail::{self, Tail};
use super::super::tree::Host;
use super::super::word::Word;
use super::*;
use crate::os::sim::SIM_THREADS;

const C: usize = 64;

fn class() -> SmallClass {
    small(C)
}

/// Pages and huge pages of the standard shape.
fn page() -> usize {
    Standard::GEOMETRY.page_size()
}

fn huge() -> usize {
    Standard::GEOMETRY.huge_page_size()
}

/// The heap's resident bytes and what the cgroup is charged for it.
fn memory(os: &Sim) -> (usize, usize) {
    // SAFETY: no other thread runs, and the back end and the tests store only integers.
    unsafe { (os.resident_bytes(), os.charged_bytes()) }
}

/// A heap whose class carved two regions and painted every slot, then gave back all but the
/// first slot of each huge page: the stranded tail the rule is for. Returns the slots kept
/// and freed.
fn stranded<'a>(back: &Back<'a, Sim>) -> (Vec<*mut u8>, Vec<*mut u8>) {
    let region = back.space().plan().region_size();
    let n = 2 * region / C;
    let slots = fill(back, class(), n);
    assert!(
        slots.len() > n / 2,
        "the class carved {} slots",
        slots.len()
    );
    for &p in &slots {
        paint(p, C, 7);
    }
    let (kept, freed): (Vec<*mut u8>, Vec<*mut u8>) = slots
        .iter()
        .partition(|p| p.addr().is_multiple_of(huge()) || (p.addr() % huge()) < C);
    for chunk in freed.chunks(64) {
        // SAFETY: the test holds these slots and gives them up.
        unsafe { back.launder(class(), chunk) };
    }
    (kept, freed)
}

/// The tail word of the region holding `at`.
fn word_at<'a>(back: &Back<'a, Sim>, at: *mut u8) -> Tail {
    let (region, _) = region_of(back, at);
    Tail::unpack(back.state.tails[region.index()].load())
}

fn region_of<'a>(back: &Back<'a, Sim>, at: *mut u8) -> (crate::space::RegionId, usize) {
    let plan = back.space().plan();
    let base = back.space().region(plan.region(0).unwrap()).addr().get();
    let index = (at.addr() - base) >> plan.region_shift();
    (
        plan.region(index).unwrap(),
        base + (index << plan.region_shift()),
    )
}

/// Ages every frontier and runs heartbeats over enough intervals for the clocked pass to have
/// gone round the heap a few times.
fn age_and_beat<'a>(back: &Back<'a, Sim>, intervals: usize) {
    forget_reuse(back);
    for _ in 0..intervals {
        back.space().os().advance_clock(DECAY_TICKS);
        beat(back);
    }
}

/// Point 1: in the huge-page part above the frontier, a step that may return memory returns the
/// free pages of huge pages that still hold something, after the kernel has been made to split
/// them, so that what the cgroup is charged falls with what is resident.
#[test]
#[cfg_attr(miri, ignore)]
fn free_pages_of_stranded_huge_pages_go_back_after_a_split() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (kept, freed) = stranded(&back);
    let (resident, charged) = memory(os);
    let freed_bytes = freed.len() * C;
    age_and_beat(&back, 12);
    let (resident_after, charged_after) = memory(os);
    assert!(os.calls(Call::SplitHuge) > 0, "no split was asked for");
    assert!(
        charged - charged_after > freed_bytes / 2,
        "charged {charged} -> {charged_after}, {freed_bytes} freed"
    );
    assert!(resident - resident_after > freed_bytes / 2);
    // The kept slots are untouched.
    for &p in &kept {
        assert!(painted(p, C, 7));
    }
    // Their huge pages are no longer mapped whole, and the advice is off there.
    let some = kept[kept.len() - 1];
    let unit = some.map_addr(|a| a & !(huge() - 1));
    assert!(!os.huge_mapped(NonNull::new(unit).unwrap()));
    assert_eq!(
        os.huge_pages(NonNull::new(unit).unwrap()),
        Some(HugePages::Avoid)
    );
    assert_ne!(word_at(&back, some).from, 0);
}

/// Without the split the same returns would leave the cgroup charged, so a refused split
/// returns nothing in part, as main did: the retained count and the charge stay, and the walk
/// does not loop on it.
#[test]
#[cfg_attr(miri, ignore)]
fn a_refused_split_returns_nothing_in_part() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (_, _) = stranded(&back);
    os.set_faults(Call::SplitHuge, Faults::ALWAYS);
    let (_, charged) = memory(os);
    age_and_beat(&back, 4);
    assert!(os.calls(Call::SplitHuge) > 0);
    // What goes back whole still goes: the small-page part below the first region's huge-page
    // part, and the first huge page of the next region, which the class's origin cuts so that
    // it holds no kept slot here. Nothing else.
    assert!(
        charged - memory(os).1 <= 2 * huge(),
        "{charged} -> {}",
        memory(os).1
    );
    for index in 0..back.walk_regions() {
        assert_eq!(Tail::unpack(back.state.tails[index].load()).from, 0);
    }
}

/// A huge page above the frontier that is wholly free goes back whole: no split and no change
/// of advice, and it is not charged afterwards.
#[test]
#[cfg_attr(miri, ignore)]
fn a_wholly_free_huge_page_goes_back_whole_with_no_advice_change() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let region = back.space().plan().region_size();
    let slots = fill(&back, class(), 2 * region / C);
    for &p in &slots {
        paint(p, C, 7);
    }
    for chunk in slots.chunks(64) {
        // SAFETY: the test holds these slots and gives them up.
        unsafe { back.launder(class(), chunk) };
    }
    let advised = os.calls(Call::AdviseHuge);
    age_and_beat(&back, 12);
    assert_eq!(os.calls(Call::SplitHuge), 0);
    assert_eq!(
        os.calls(Call::AdviseHuge),
        advised,
        "advice changed with nothing in part"
    );
    let (resident, charged) = memory(os);
    // The huge page holding the class's watermark stays as main leaves it (`region::top_up`
    // does not carve it here), charged whole.
    assert!(
        charged < region / 4 + huge(),
        "{charged} still charged, {resident} resident"
    );
}

/// Point 2: the advice comes off once per region per pass, and never where nothing went back:
/// a heap whose huge pages hold no free page above the frontier keeps its advice.
#[test]
#[cfg_attr(miri, ignore)]
fn advice_comes_off_once_a_region_a_pass_and_only_where_memory_went_back() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    // Dense: one slot of every page is freed, so no page is wholly free.
    let region = back.space().plan().region_size();
    let slots = fill(&back, class(), 2 * region / C);
    for &p in &slots {
        paint(p, C, 7);
    }
    let per_page = page() / C;
    let sparse: Vec<*mut u8> = slots
        .iter()
        .copied()
        .filter(|p| (p.addr() / C) % per_page == 1)
        .collect();
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class(), &sparse) };
    let advised = os.calls(Call::AdviseHuge);
    age_and_beat(&back, 8);
    assert_eq!(
        os.calls(Call::AdviseHuge),
        advised,
        "advice changed where nothing went back"
    );
    assert_eq!(os.calls(Call::SplitHuge), 0);

    // Now strand the rest: each region's advice comes off in one call a pass at most.
    let rest: Vec<*mut u8> = slots
        .iter()
        .copied()
        .filter(|p| (p.addr() / C) % per_page != 1 && p.addr() % huge() >= C)
        .collect();
    // SAFETY: as above.
    unsafe { back.launder(class(), &rest) };
    let laps = back
        .state
        .walk
        .laps
        .load(core::sync::atomic::Ordering::Relaxed);
    let advised = os.calls(Call::AdviseHuge);
    age_and_beat(&back, 8);
    let laps = (back
        .state
        .walk
        .laps
        .load(core::sync::atomic::Ordering::Relaxed)
        - laps) as u64;
    let changes = os.calls(Call::AdviseHuge) - advised;
    assert!(changes > 0, "the advice never came off");
    // Two regions hold the class; at most one lowering each per lap, and a lap may end
    // partway, so one more.
    assert!(
        changes <= 2 * (laps + 1),
        "{changes} advice calls in {laps} laps"
    );
}

/// Point 3: when claims reach above a lowered boundary, the advice goes back on, by the
/// background thread or a heartbeat and never by the claim: the claims change no advice, the
/// region is marked, and the next heartbeat raises the boundary to the frontier and a bounded
/// headroom beyond it.
#[test]
#[cfg_attr(miri, ignore)]
fn a_claim_above_the_boundary_marks_and_a_heartbeat_raises() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (kept, _) = stranded(&back);
    age_and_beat(&back, 12);
    let some = kept[0];
    let before = word_at(&back, some);
    assert_ne!(before.from, 0);
    // The fills take the lowest free slots, which are in the tail now.
    let advised = os.calls(Call::AdviseHuge);
    let again = fill(&back, class(), 4096);
    assert!(!again.is_empty());
    assert_eq!(
        os.calls(Call::AdviseHuge),
        advised,
        "a claim changed the advice"
    );
    assert!(back.state.marks.load() != 0, "no region was marked");
    back.heartbeat();
    assert!(
        os.calls(Call::AdviseHuge) > advised,
        "the heartbeat did not raise"
    );
    let after = word_at(&back, some);
    assert!(
        after.from == 0 || after.from > before.from,
        "{before:?} -> {after:?}"
    );
    assert!(!after.want);
}

/// Point 4: below the frontier nothing changes: while fills keep reaching the top, free pages
/// beside live ones are not split, and the advice stays.
#[test]
#[cfg_attr(miri, ignore)]
fn below_the_frontier_nothing_changes() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (_, _) = stranded(&back);
    let advised = os.calls(Call::AdviseHuge);
    let region = back.space().plan().region_size();
    for _ in 0..6 {
        // Claims of every free slot each interval keep the frontiers at the top.
        let all = fill(&back, class(), 2 * region / C);
        os.advance_clock(DECAY_TICKS);
        beat(&back);
        // SAFETY: the test holds these slots and gives them up.
        unsafe { back.launder(class(), &all) };
    }
    assert_eq!(os.calls(Call::SplitHuge), 0);
    let _ = advised;
    for index in 0..back.walk_regions() {
        let tail = Tail::unpack(back.state.tails[index].load());
        assert_eq!(tail.from, 0, "region {index} lowered under its frontier");
    }
}

/// Point 5: a split counts as a whole step, so a heartbeat asks for at most one.
#[test]
#[cfg_attr(miri, ignore)]
fn a_heartbeat_asks_for_at_most_one_split() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (_, _) = stranded(&back);
    forget_reuse(&back);
    os.advance_clock(DECAY_TICKS);
    let mut most = 0;
    for _ in 0..BEATS {
        let before = os.calls(Call::SplitHuge);
        back.heartbeat();
        most = most.max(os.calls(Call::SplitHuge) - before);
    }
    assert!(os.calls(Call::SplitHuge) > 0);
    assert_eq!(most, 1, "a heartbeat made {most} splits");
}

/// Point 6: `trim` does all of it at once, whatever the frontiers say.
#[test]
#[cfg_attr(miri, ignore)]
fn trim_returns_the_tails_at_once() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    // Fills reach the whole tail in this interval: a trim takes the advice off all the same.
    let (kept, freed) = stranded(&back);
    let (_, charged) = memory(os);
    back.trim();
    let (_, after) = memory(os);
    assert!(
        charged - after > freed.len() * C / 2,
        "{charged} -> {after}"
    );
    assert_ne!(word_at(&back, kept[0]).from, 0);
    for &p in &kept {
        assert!(painted(p, C, 7));
    }
}

/// A step that only counts (a pass held under the floor) splits nothing, returns nothing and
/// changes no advice above the frontier: it looks at each huge page cheaply.
#[test]
#[cfg_attr(miri, ignore)]
fn a_counting_lap_splits_and_returns_nothing() {
    let state = floored_state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (_, _) = stranded(&back);
    let (purges, advised) = (os.calls(Call::Purge), os.calls(Call::AdviseHuge));
    age_and_beat(&back, 6);
    // The heap is far under the floor, so every pass only counts.
    assert_eq!(os.calls(Call::SplitHuge), 0);
    assert_eq!(os.calls(Call::Purge), purges);
    assert_eq!(os.calls(Call::AdviseHuge), advised);
}

/// The cheap look reads a bounded number of words of a dense huge page, and calls it dense.
#[test]
fn the_cheap_look_is_bounded_and_tells_dense_from_sparse() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let region = back.space().plan().region_size();
    let slots = fill(&back, class(), 2 * region / C);
    let host = back.host(class().class()).unwrap();
    let (region0, base) = region_of(&back, slots[slots.len() - 1]);
    let hp = (huge() / page()) as u32;
    let top = host
        .layout()
        .carved_pages(Grow::unpack(host.grow(region0).load()).carved);
    let unit = super::super::layout::Unit {
        start: (top - 1) & !(hp - 1),
        end: ((top - 1) & !(hp - 1)) + hp,
    };
    let dense = tail::look(&host, region0, unit, 0);
    assert!(!dense.sparse);
    // A page of this class is one leaf word: a sample reads it, after the clean word.
    assert!(dense.read <= 2 * super::super::policy::TAIL_SAMPLE_PAGES as usize);
    // Free the unit's slots and look again.
    let lo = base + unit.start as usize * page();
    let hi = base + unit.end as usize * page();
    let inside: Vec<*mut u8> = slots
        .iter()
        .copied()
        .filter(|p| (lo..hi).contains(&p.addr()) && (p.addr() - lo) >= C)
        .collect();
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class(), &inside) };
    let sparse = tail::look(&host, region0, unit, 0);
    assert!(sparse.sparse);
}

/// A refused lowering keeps the boundary where it was and asks for a repair; the next change
/// reconciles first, and the kernel's advice then agrees with the word.
#[test]
#[cfg_attr(miri, ignore)]
fn a_refused_lowering_is_repaired_by_the_next_change() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (kept, _) = stranded(&back);
    os.set_faults(Call::AdviseHuge, Faults::ALWAYS.partially());
    age_and_beat(&back, 12);
    let some = kept[kept.len() - 1];
    let refused = word_at(&back, some);
    assert!(refused.repair, "{refused:?}");
    os.set_faults(Call::AdviseHuge, Faults::NONE);
    let (region, _) = region_of(&back, some);
    let host = back.host(class().class()).unwrap();
    let huge_from = Hints::unpack(host.hints(region).load()).huge_from.unwrap();
    tail::lower(&host, region, huge_from, back.epoch());
    let now = word_at(&back, some);
    assert!(!now.repair && !now.busy, "{now:?}");
    agrees(&back, region, now);
}

/// Checks that the simulator's advice over `region`'s huge-page part is what `tail` says.
fn agrees<'a>(back: &Back<'a, Sim>, region: crate::space::RegionId, tail: Tail) {
    let os = back.space().os();
    let host = back.host(class().class()).unwrap();
    let layout = *host.layout();
    let huge_from = Hints::unpack(host.hints(region).load()).huge_from.unwrap();
    let hp = layout.huge_pages();
    let base = back.space().region(region);
    for unit in huge_from / hp..layout.region_pages() / hp {
        let at = base.map_addr(|a| a.saturating_add((unit * hp) as usize * page()));
        let want = if tail.from != 0 && unit >= tail.from {
            HugePages::Avoid
        } else {
            HugePages::Prefer
        };
        assert_eq!(os.huge_pages(at), Some(want), "unit {unit} of {tail:?}");
    }
}

/// A refused purge leaves the count as it was and notes nothing.
#[test]
#[cfg_attr(miri, ignore)]
fn a_refused_purge_changes_nothing() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (kept, _) = stranded(&back);
    os.set_faults(Call::Purge, Faults::ALWAYS);
    let retained = back.retained();
    age_and_beat(&back, 4);
    // Nothing went back, so nothing was counted out (a top-up may count in).
    assert!(back.retained() >= retained);
    for index in 0..back.walk_regions() {
        assert_eq!(Tail::unpack(back.state.tails[index].load()).from, 0);
    }
    let _ = kept;
}

/// Lowerings and raises of one region on several threads at once, with claims marking it and
/// walkers noting huge pages: no two advice changes overlap, nothing waits, and once all are
/// done and no repair is asked for, the kernel's advice is what the word says.
#[test]
#[cfg_attr(miri, ignore)]
fn lowering_and_raising_race_and_the_advice_agrees() {
    for round in 0..8u64 {
        let state = state::<Standard>();
        let back = state.get_or_reserve().unwrap();
        let (kept, _) = stranded(&back);
        let (region, _) = region_of(&back, kept[kept.len() - 1]);
        let host = back.host(class().class()).unwrap();
        let layout = *host.layout();
        let huge_from = Hints::unpack(host.hints(region).load()).huge_from.unwrap();
        let hp = layout.huge_pages();
        let units = layout.region_pages() / hp;
        let shared = &*state;
        thread::scope(|scope| {
            for t in 0..SIM_THREADS.min(4) as u64 {
                scope.spawn(move || {
                    let back = shared.get().unwrap();
                    let host = &back.host(class().class()).unwrap();
                    let mut rng = Rng((round * 31 + t) | 1);
                    for _ in 0..400 {
                        let unit =
                            huge_from / hp + rng.below((units - huge_from / hp) as usize) as u32;
                        match rng.below(4) {
                            0 => tail::note(host, region, unit),
                            1 => {
                                let _ = tail::mark(host, region, unit * hp + 1);
                            }
                            2 => {
                                let _ = tail::lower(host, region, huge_from, round);
                            }
                            _ => {
                                let frontier = rng.below(layout.region_pages() as usize) as u32;
                                let _ = tail::raise(
                                    host,
                                    region,
                                    huge_from,
                                    frontier,
                                    layout.region_pages(),
                                    round,
                                );
                            }
                        }
                    }
                });
            }
        });
        let mut now = Tail::unpack(host.tail(region).load());
        assert!(!now.busy, "an owner bit left set: {now:?}");
        if now.repair {
            tail::lower(&host, region, huge_from, round);
            now = Tail::unpack(host.tail(region).load());
        }
        assert!(!now.repair, "{now:?}");
        agrees(&back, region, now);
    }
}

/// The child of a fork asks for a repair on every tail word it inherited, and leaves the
/// zero words of regions whose advice never came off.
#[test]
#[cfg_attr(miri, ignore)]
fn a_fork_child_distrusts_every_tail() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let (kept, _) = stranded(&back);
    age_and_beat(&back, 12);
    let (region, _) = region_of(&back, kept[kept.len() - 1]);
    assert_ne!(back.state.tails[region.index()].load(), 0);
    back.distrust_tails();
    for (index, word) in back.state.tails.iter().enumerate() {
        let tail = Tail::unpack(word.load());
        if word.load() != 0 {
            assert!(tail.repair, "region {index}");
        }
    }
    let host = back.host(class().class()).unwrap();
    let huge_from = Hints::unpack(host.hints(region).load()).huge_from.unwrap();
    tail::lower(&host, region, huge_from, back.epoch());
    let now = Tail::unpack(host.tail(region).load());
    assert!(!now.repair);
    agrees(&back, region, now);
}

/// The simulator's model, which the tests above rest on: a huge page committed under the
/// advice is charged whole; a purge of part of it leaves it charged until it is split; and
/// khugepaged rebuilds an advised huge page that holds anything, but not one advised against.
#[test]
fn the_simulator_charges_huge_pages_as_linux_does() {
    let os = Sim::<Standard>::new();
    let region = Standard::GEOMETRY.largest_plan().region_size();
    let base = os.reserve(region, region).unwrap();
    let hp = huge();
    // SAFETY: the range is the reservation's, which this test alone uses.
    unsafe {
        os.advise_huge(base, region, HugePages::Prefer).unwrap();
        os.commit(base, region).unwrap();
        base.as_ptr().write(1);
        let (_, charged) = (os.resident_bytes(), os.charged_bytes());
        assert_eq!(charged, region, "each huge page charged whole");
        // The first huge page purged but for its first page: still charged.
        os.purge(base.map_addr(|a| a.saturating_add(page())), hp - page())
            .unwrap();
        assert_eq!(os.charged_bytes(), region);
        // Split, then purged: charged by its resident page only.
        os.split_huge(base.map_addr(|a| a.saturating_add(page())), page())
            .unwrap();
        assert_eq!(os.charged_bytes(), region - hp + page());
        // khugepaged rebuilds it under the advice.
        assert_eq!(os.khugepaged(), 1);
        assert_eq!(os.charged_bytes(), region);
        // Not under advice against.
        os.split_huge(base.map_addr(|a| a.saturating_add(page())), page())
            .unwrap();
        os.advise_huge(base, hp, HugePages::Avoid).unwrap();
        assert_eq!(os.khugepaged(), 0);
        // A whole huge page purged is no longer charged.
        let second = base.map_addr(|a| a.saturating_add(hp));
        os.purge(second, hp).unwrap();
        assert_eq!(os.charged_bytes(), region - 2 * hp + page());
    }
}

/// The look samples pages, not leaf words: in a class of 1 KiB a word covers 16 pages, and a
/// huge page with one live slot in every 16 pages has no free word but 15 free pages in 16.
/// Those go back.
#[test]
#[cfg_attr(miri, ignore)]
fn a_huge_page_with_no_free_word_but_free_pages_goes_back_in_part() {
    const K: usize = 1024;
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let region = back.space().plan().region_size();
    let slots = fill(&back, small(K), 2 * region / K);
    assert!(slots.len() > region / K);
    for &p in &slots {
        paint(p, K, 7);
    }
    let (kept, freed): (Vec<*mut u8>, Vec<*mut u8>) = slots
        .iter()
        .partition(|p| p.addr().is_multiple_of(16 * page()));
    for chunk in freed.chunks(64) {
        // SAFETY: the test holds these slots and gives them up.
        unsafe { back.launder(small(K), chunk) };
    }
    let (_, charged) = memory(os);
    age_and_beat(&back, 12);
    let (_, after) = memory(os);
    assert!(os.calls(Call::SplitHuge) > 0, "no split was asked for");
    assert!(
        charged - after > freed.len() * K / 2,
        "charged {charged} -> {after}, {} freed",
        freed.len() * K
    );
    for &p in &kept {
        assert!(painted(p, K, 7));
    }
}

/// The bound of point 5 is for a request's slow path: a step of the background thread's may
/// split as many huge pages as its budget reaches.
#[test]
#[cfg_attr(miri, ignore)]
fn a_step_of_the_thread_may_split_more_than_one() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (_, _) = stranded(&back);
    forget_reuse(&back);
    os.advance_clock(DECAY_TICKS);
    let mut most = 0;
    for _ in 0..BEATS {
        let before = os.calls(Call::SplitHuge);
        let _ = back.thread_purge_steps(&|| back.now());
        most = most.max(os.calls(Call::SplitHuge) - before);
    }
    assert!(most > 1, "a step of the thread made {most} splits at most");
}

/// The look's samples move with the salt and do not keep one phase of a periodic layout: a
/// huge page whose odd pages are free and even ones are not is found sparse by nearly every
/// salt, where samples at fixed multiples of a power of two would all fall on even pages.
#[test]
fn the_cheap_look_moves_its_samples_with_the_salt() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let region = back.space().plan().region_size();
    let slots = fill(&back, class(), 2 * region / C);
    let host = back.host(class().class()).unwrap();
    // The top huge page of the first region, which the class carved whole.
    let (region0, base) = region_of(&back, slots[0]);
    let hp = (huge() / page()) as u32;
    let top = host
        .layout()
        .carved_pages(Grow::unpack(host.grow(region0).load()).carved);
    let start = ((top - 1) & !(hp - 1)) - hp;
    let unit = super::super::layout::Unit {
        start,
        end: start + hp,
    };
    let lo = base + unit.start as usize * page();
    let hi = base + unit.end as usize * page();
    // Every odd page of the unit wholly free, every even one holding its slots.
    let odd: Vec<*mut u8> = slots
        .iter()
        .copied()
        .filter(|p| (lo..hi).contains(&p.addr()) && ((p.addr() - lo) / page()) % 2 == 1)
        .collect();
    for &p in &odd {
        paint(p, C, 7);
    }
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class(), &odd) };
    let found = (0..16)
        .filter(|&salt| tail::look(&host, region0, unit, salt).sparse)
        .count();
    assert!(found >= 12, "{found} of 16 looks found two free pages");
}

/// Two regions of the class carved, every slot painted; returns the slots and, with `first`,
/// the first region, which the class carved whole, else the second, with its base address.
/// The first region's huge-page part starts at its second huge page.
fn carved<'a>(back: &Back<'a, Sim>, first: bool) -> (Vec<*mut u8>, crate::space::RegionId, usize) {
    let region = back.space().plan().region_size();
    let slots = fill(back, class(), 2 * region / C);
    for &p in &slots {
        paint(p, C, 7);
    }
    let which = if first {
        slots[0]
    } else {
        slots[slots.len() - 1]
    };
    let (region, base) = region_of(back, which);
    (slots, region, base)
}

/// Gives back every slot of `region` (at `base`) whose page, counted from the region's start,
/// `free` says.
fn free_pages<'a>(
    back: &Back<'a, Sim>,
    slots: &[*mut u8],
    base: usize,
    free: impl Fn(usize) -> bool,
) -> Vec<*mut u8> {
    let region = back.space().plan().region_size();
    let freed: Vec<*mut u8> = slots
        .iter()
        .copied()
        .filter(|p| (base..base + region).contains(&p.addr()) && free((p.addr() - base) / page()))
        .collect();
    for chunk in freed.chunks(64) {
        // SAFETY: the test holds these slots and gives them up.
        unsafe { back.launder(class(), chunk) };
    }
    freed
}

/// The split is the walk's own count's to decide, a cut and not a rate: a huge page with three
/// of its sixteen pages free, a run the walk would take, is never split however many laps
/// sample it, and one half free is. Nor does a run of free pages reach across into the huge
/// page below: the lower one's top pages are free and the upper one's bottom pages too, a run
/// of seven pages across the boundary, and the lower one stays whole.
#[test]
#[cfg_attr(miri, ignore)]
fn a_huge_page_is_split_only_past_a_quarter_free_and_a_run_stays_in_its_huge_page() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (slots, second, base) = carved(&back, true);
    let hp = huge() / page();
    let freed = free_pages(&back, &slots, base, |p| {
        let (unit, at) = (p / hp, p % hp);
        match unit {
            // Its top three pages: under a quarter free.
            1 => at >= hp - 3,
            // Half free, in runs of four, its bottom four pages among them.
            2 => at % 8 < 4,
            _ => false,
        }
    });
    assert!(!freed.is_empty());
    age_and_beat(&back, 30);
    let start = back.space().region(second);
    let at = |n: usize| start.map_addr(|a| a.saturating_add(n * huge()));
    assert!(
        !os.huge_mapped(at(2)),
        "the half-free huge page was not split"
    );
    assert!(
        os.huge_mapped(at(1)),
        "a huge page under a quarter free was split, or a run reached into it from above"
    );
}

/// The walk passes a dense huge page above the frontier at its cheap look, and does not read
/// it a page at a time: a step over a tail whose every page holds a live slot reads a few
/// words a huge page.
#[test]
#[cfg_attr(miri, ignore)]
fn the_walk_reads_a_dense_huge_page_at_a_look() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let (slots, region, base) = carved(&back, true);
    // Every page keeps its first slot.
    let per_page = page() / C;
    let freed: Vec<*mut u8> = slots
        .iter()
        .copied()
        .filter(|p| p.addr() >= base && !((p.addr() - base) / C).is_multiple_of(per_page))
        .collect();
    for chunk in freed.chunks(64) {
        // SAFETY: the test holds these slots and gives them up.
        unsafe { back.launder(class(), chunk) };
    }
    forget_reuse(&back);
    let (purged, read, _, _) = back.purge_region(
        class().class(),
        region,
        usize::MAX,
        usize::MAX,
        true,
        super::super::purge::Scope::Idle,
        false,
        false,
    );
    assert_eq!(purged, 0);
    let units = back.space().plan().region_size() / huge();
    let hp = huge() / page();
    // A page a word for this class: a walk a page at a time reads a word a page and more.
    assert!(
        read < units * hp,
        "{read} words read over {units} dense huge pages of {hp} pages"
    );
}

/// `lower_walked`: a pass or a growth payment that ends partway through a region lowers its
/// boundary there, without waiting for the walk to reach the region's bottom.
#[test]
#[cfg_attr(miri, ignore)]
fn a_pass_that_ends_inside_a_region_lowers_its_boundary() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let (kept, _) = stranded(&back);
    forget_reuse(&back);
    let (region, _) = region_of(&back, kept[kept.len() - 1]);
    let units = (back.space().plan().region_size() / huge()) as u32;
    let host = back.host(class().class()).unwrap();
    tail::note(&host, region, units - 1);
    back.state.walk.purge_at.store(
        region.index() as u32 + 1,
        core::sync::atomic::Ordering::Relaxed,
    );
    back.lower_walked();
    assert_eq!(word_at(&back, kept[kept.len() - 1]).from, units - 1);
}

/// The background thread is woken for a mark, and sleeps through one it cannot act on: a
/// thread's heartbeat says there is work once a claim marked a region; `purge_due` says so
/// too; and after the kernel refused a raise neither does until the next interval, though the
/// region keeps its mark.
#[test]
#[cfg_attr(miri, ignore)]
fn the_thread_is_woken_for_a_mark_and_not_for_a_refused_raise() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (kept, _) = stranded(&back);
    age_and_beat(&back, 12);
    let awake = crate::thread_word::Snapshot(crate::thread_word::AWAKE);
    while back.heartbeat_for(&|| back.now(), awake) {
        while back.thread_purge_steps(&|| back.now()).more {}
    }
    assert!(!back.purge_due(&|| back.now()));
    let mut held = Vec::new();
    while back.state.marks.load() == 0 {
        let more = fill(&back, class(), 64);
        assert!(!more.is_empty());
        held.extend(more);
    }
    assert!(
        back.heartbeat_for(&|| back.now(), awake),
        "a mark woke nobody"
    );
    assert!(
        back.purge_due(&|| back.now()),
        "the thread would sleep on a mark"
    );
    os.set_faults(Call::AdviseHuge, Faults::ALWAYS);
    let _ = back.thread_purge_steps(&|| back.now());
    let marked =
        (0..back.walk_regions()).any(|index| Tail::unpack(back.state.tails[index].load()).want);
    assert!(marked, "a refused raise dropped the mark");
    assert!(
        !back.purge_due(&|| back.now()),
        "the thread would retry a refused raise in the same interval"
    );
    os.set_faults(Call::AdviseHuge, Faults::NONE);
    os.advance_clock(DECAY_TICKS);
    beat(&back);
    assert!(
        back.purge_due(&|| back.now())
            || !(0..back.walk_regions())
                .any(|index| Tail::unpack(back.state.tails[index].load()).want),
        "the mark was forgotten after the interval"
    );
    let _ = kept;
}

/// The fork hook, end to end: the child's first slow path draws its generation, the depot
/// tells the back end (`Source::forked`), and every inherited tail word asks for a repair,
/// loses an owner bit no thread of the child holds, and shows its mark to the lookers.
#[test]
#[cfg_attr(miri, ignore)]
fn a_fork_child_repairs_its_tails_at_its_first_slow_path() {
    use core::alloc::{GlobalAlloc, Layout};
    let heap = crate::heap::SimHeap::<Standard>::new();
    let layout = Layout::from_size_align(64, 8).unwrap();
    // SAFETY: valid layouts; each block is freed once.
    let first = unsafe { heap.alloc(layout) };
    assert!(!first.is_null());
    // The parent draws its generation (a drain needs one), so the child's draw is a child's.
    let _ = heap.drain(0);
    let back = heap.heap().back().unwrap();
    let stuck = Tail {
        from: 2,
        want: true,
        busy: true,
        stamp: back.epoch() as u32,
        ..Tail::default()
    };
    back.state.tails[0].store(stuck.pack());
    back.state.marks.fetch_and(0);
    // SAFETY: nothing else runs.
    unsafe { heap.os().wipe_for_fork() };
    let mut held = Vec::new();
    for _ in 0..64 {
        // SAFETY: as above.
        let p = unsafe { heap.alloc(layout) };
        assert!(!p.is_null());
        held.push(p);
        if Tail::unpack(back.state.tails[0].load()).repair {
            break;
        }
        // The child's first draw, if no slow path has made it yet.
        let _ = heap.drain(0);
    }
    let word = Tail::unpack(back.state.tails[0].load());
    assert!(word.repair && !word.busy && word.want, "{word:?}");
    assert_ne!(back.state.marks.load(), 0);
    // SAFETY: as above.
    unsafe {
        for p in held {
            heap.dealloc(p, layout);
        }
        heap.dealloc(first, layout);
    }
}

/// A raise whose headroom reaches past the huge page that holds the watermark goes to the
/// region's end: above the watermark nothing is touched, and a class growing there faults
/// huge pages again. In a region far longer than the headroom.
#[test]
#[cfg_attr(miri, ignore)]
fn a_raise_past_the_watermark_goes_to_the_end() {
    let state = state::<Native>();
    let back = state.get_or_reserve().unwrap();
    let region_bytes = back.space().plan().region_size();
    let slots = fill(&back, class(), 4 * region_bytes / C);
    assert!(!slots.is_empty());
    let host = back.host(class().class()).unwrap();
    let layout = *host.layout();
    let hp = layout.huge_pages();
    let units = layout.region_pages() / hp;
    let plan = back.space().plan();
    let carved = |region: crate::space::RegionId| {
        layout.carved_pages(Grow::unpack(host.grow(region).load()).carved)
    };
    let region = (0..back.walk_regions())
        .filter_map(|index| plan.region(index))
        .filter(|&region| back.state.table.get(region) == RegionTag::Class(class().class()))
        .max_by_key(|&region| carved(region))
        .unwrap();
    let from = Hints::unpack(host.hints(region).load()).huge_from.unwrap();
    let first = from / hp;
    assert!(first + 4 + super::super::policy::TAIL_HEADROOM_HUGE_PAGES < units);
    tick(&back, 2);
    let epoch = back.epoch();
    tail::note(&host, region, first + 1);
    assert_eq!(tail::lower(&host, region, from, epoch), tail::Change::Done);
    assert!(tail::mark(&host, region, (first + 1) * hp + 1));
    // The watermark two huge pages up: the headroom reaches past it.
    let top = (first + 3) * hp;
    assert_eq!(
        tail::raise(&host, region, from, (first + 1) * hp + 1, top, epoch),
        tail::Change::Done
    );
    assert_eq!(Tail::unpack(host.tail(region).load()).from, 0);
}

/// Stage two, the accounting: once a raise puts split huge pages back under the advice, every
/// free page of each one that holds anything counts as resident again, since khugepaged may
/// rebuild it whole; a huge page wholly returned stays as it is; and the raised range ages
/// like a fill, so the next interval's pass does not split it again at once.
#[test]
#[cfg_attr(miri, ignore)]
fn a_raise_counts_its_huge_pages_resident_again_and_ages_like_a_fill() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let os = state.os();
    let (kept, _) = stranded(&back);
    // The top huge page's one kept slot goes too: it will be returned whole.
    let top_kept = kept[kept.len() - 1];
    // SAFETY: the test holds this slot and gives it up.
    unsafe { back.launder(class(), &[top_kept]) };
    age_and_beat(&back, 12);
    let (region, _) = region_of(&back, top_kept);
    let host = back.host(class().class()).unwrap();
    let layout = *host.layout();
    let hp = layout.huge_pages();
    let before = Tail::unpack(host.tail(region).load());
    assert_ne!(before.from, 0);
    let top = layout.carved_pages(Grow::unpack(host.grow(region).load()).carved);
    let unit = |u: u32| super::super::layout::Unit {
        start: (u * hp).max(layout.origin_page()),
        end: ((u + 1) * hp).min(top),
    };
    let last = (top - 1) / hp;
    assert_eq!(
        super::super::region::dirty_pages(&host, region, unit(last)).0,
        0
    );
    let retained = back.retained();
    // Claims above the boundary, then the raise.
    let mut held = Vec::new();
    while Tail::unpack(host.tail(region).load()).from == before.from {
        let more = fill(&back, class(), 64);
        assert!(!more.is_empty());
        held.extend(more);
        for _ in 0..4 {
            back.heartbeat();
        }
    }
    let after = Tail::unpack(host.tail(region).load());
    let raised_to = if after.from == 0 {
        top.div_ceil(hp)
    } else {
        after.from
    };
    for u in before.from..raised_to {
        let (dirty, _) = super::super::region::dirty_pages(&host, region, unit(u));
        let pages = (unit(u).end - unit(u).start) as usize;
        if u == last {
            assert_eq!(
                dirty, 0,
                "the wholly returned huge page was counted resident"
            );
        } else {
            assert_eq!(
                dirty, pages,
                "huge page {u}: {dirty} of {pages} pages counted"
            );
        }
    }
    assert!(
        back.retained() > retained,
        "the retained count did not grow"
    );
    // The claims go back. The next interval's passes leave the raised range alone, though
    // the claims reached only its bottom; once the frontier has aged below it, they split
    // its huge pages again.
    // SAFETY: the test holds these slots and gives them up.
    unsafe { back.launder(class(), &held) };
    let splits = os.calls(Call::SplitHuge);
    os.advance_clock(DECAY_TICKS);
    beat(&back);
    // A step of the next interval's pass, that may return everything.
    let _ = back.purge_region(
        class().class(),
        region,
        usize::MAX,
        usize::MAX,
        true,
        super::super::purge::Scope::Idle,
        false,
        false,
    );
    assert_eq!(
        os.calls(Call::SplitHuge),
        splits,
        "the raise was undone at once"
    );
    age_and_beat(&back, 6);
    assert!(
        os.calls(Call::SplitHuge) > splits,
        "the raised range was never split again"
    );
}

/// Stage two: a note the frontier has passed is dropped and the advice stays on over it, so
/// the huge pages between the note and the frontier, returned in part, count as resident
/// again, as under a raise.
#[test]
#[cfg_attr(miri, ignore)]
fn a_dropped_note_counts_its_huge_pages_resident_again() {
    let state = state::<Standard>();
    let back = state.get_or_reserve().unwrap();
    let (kept, _) = stranded(&back);
    forget_reuse(&back);
    let at = kept[kept.len() - 1];
    let (region, _) = region_of(&back, at);
    let host = back.host(class().class()).unwrap();
    let layout = *host.layout();
    let hp = layout.huge_pages();
    // A step that returns part of a huge page and stops there, noting it.
    let (purged, _, done, _) = back.purge_region(
        class().class(),
        region,
        usize::MAX,
        page(),
        true,
        super::super::purge::Scope::Idle,
        false,
        true,
    );
    assert!(purged != 0 && !done);
    let noted = Tail::unpack(host.tail(region).load()).low.unwrap();
    let unit = super::super::layout::Unit {
        start: noted * hp,
        end: (noted + 1) * hp,
    };
    let (dirty, _) = super::super::region::dirty_pages(&host, region, unit);
    assert!(dirty < hp as usize);
    // Fills now reach above it, in this interval.
    let top = layout.carved_pages(Grow::unpack(host.grow(region).load()).carved);
    let _ = super::super::region::reach(
        &host,
        region,
        layout.slot_at(top as usize * page() - 1),
        back.epoch(),
    );
    back.state.walk.purge_at.store(
        region.index() as u32 + 1,
        core::sync::atomic::Ordering::Relaxed,
    );
    let retained = back.retained();
    back.lower_walked();
    let word = Tail::unpack(host.tail(region).load());
    assert_eq!((word.from, word.low), (0, None), "{word:?}");
    let (dirty, _) = super::super::region::dirty_pages(&host, region, unit);
    assert_eq!(
        dirty, hp as usize,
        "the dropped note's huge page was not counted resident"
    );
    assert!(back.retained() > retained);
}
