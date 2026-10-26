# The switch's edits to the allocator, run by ht_edit.py with `sub`, `SRC`, `TREE` and `HERE`
# in scope. See ht.rs for what the rule does and how `ASMALLOC_HT` chooses the variant.

# --- A third kind of advice: ask the kernel to split the huge page under a range
# (MADV_COLD on part of a huge page splits it; docs/research/huge-tail.md).
sub("os/mod.rs", """    /// Never back the range with huge pages (`MADV_NOHUGEPAGE`).
    Avoid,
}""", """    /// Never back the range with huge pages (`MADV_NOHUGEPAGE`).
    Avoid,
    /// Scratch (huge-tail): split any huge page the range covers in part (`MADV_COLD`).
    Split,
}""")
sub("os/linux/kernel.rs", "const MADV_NOHUGEPAGE: usize = 15;\n",
    "const MADV_NOHUGEPAGE: usize = 15;\nconst MADV_COLD: usize = 20;\n")
sub("os/linux/kernel.rs", """            HugePages::Avoid => MADV_NOHUGEPAGE,
        };""", """            HugePages::Avoid => MADV_NOHUGEPAGE,
            HugePages::Split => MADV_COLD,
        };""")
sub("os/sim/mod.rs", """            HugePages::Avoid => self.pages.update(first, applied, NO_HUGE, HUGE),
        }""", """            HugePages::Avoid => self.pages.update(first, applied, NO_HUGE, HUGE),
            HugePages::Split => {}
        }""")

# --- A claim above a region's tail puts the advice back (or marks the region for a heartbeat).
sub("back/region.rs", """    let format = layout.frontier_format();
    let word = host.frontier(region);
    let mut seen = word.load();
    let first = !format.claimed_in(seen, epoch);""", """    crate::ht::note_reach(region.index(), page);
    if let Some((from, to)) =
        crate::ht::reached(region.index(), page, layout.region_pages(), layout.huge_pages())
    {
        ht_readvise(host, region, from, to);
    }
    let format = layout.frontier_format();
    let word = host.frontier(region);
    let mut seen = word.load();
    let first = !format.claimed_in(seen, epoch);""")
sub("back/region.rs", """/// The page below which `region`'s reuse reached""", """/// Scratch (huge-tail): puts the huge-page advice back on pages `from..to` of `region`.
#[cold]
#[inline(never)]
fn ht_readvise<H: Host>(host: &H, region: RegionId, from: u32, to: u32) {
    let page = host.layout().page_size();
    let t0 = crate::ht::cycles();
    host.advise(
        region,
        from as usize * page,
        (to - from) as usize * page,
        HugePages::Prefer,
    );
    crate::ht::timed(&crate::ht::READVISE_CYC, Some(&crate::ht::READVISE_MAX), t0);
    crate::ht::count(&crate::ht::READVISE, 1);
    crate::ht::forget_splits(region.index());
}

/// The page below which `region`'s reuse reached""")

# --- The walk: advise the tail against, and walk it as small pages.
P = "back/purge.rs"
sub(P, "use super::layout::{Hints, Step};\n", "use super::layout::{Grow, Hints, Layout, Step};\nuse crate::os::HugePages;\n")
sub(P, """        if cursor != 0 && cursor <= floor {
            // Everything the cursor has left is below the frontier.""", """        // Scratch (huge-tail): the part of the huge-page part above the aged frontier, rounded
        // up to a huge page, is advised against huge pages again when it is long enough.
        let ht_on = crate::ht::config().on;
        // With `g1`, only a step that may return something lowers a boundary: a pass held
        // under the floor only counts, and lowering the advice then buys nothing.
        let ht_goal = !crate::ht::config().goal || (limit > 0 && !counting);
        if ht_on && resume && ht_goal && !crate::ht::config().lazy {
            if let Some(huge) = Hints::unpack(hints.load()).huge_from {
                let fr = region::floor(&host, region, self.epoch());
                let fr = crate::ht::patient(region.index(), fr, self.epoch());
                let g = layout.huge_pages();
                let from = fr.map_or(u32::MAX, |fr| ((fr + g - 1) & !(g - 1)).max(huge));
                let top = (layout.carved_pages(Grow::unpack(grow.load()).carved) + g - 1) & !(g - 1);
                if from < layout.region_pages()
                    && crate::ht::may_unadvise(region.index(), from, top, layout.page_shift(), self.epoch())
                {
                    let page = layout.page_size();
                    let t0 = crate::ht::cycles();
                    if host.advise(
                        region,
                        from as usize * page,
                        (layout.region_pages() - from) as usize * page,
                        HugePages::Avoid,
                    ) {
                        crate::ht::set_tail(region.index(), from);
                    }
                    crate::ht::timed(&crate::ht::UNADVISE_CYC, Some(&crate::ht::UNADVISE_MAX), t0);
                }
            }
        }
        let tail = if ht_on { crate::ht::tail_of(region.index()) } else { None };
        // With `r1` the advice comes off only where a step returns part of a unit: a step
        // that may return walks the units above the frontier as small pages (the zone), and
        // lowers the boundary afterwards, in one call, to the lowest unit it split.
        let ht_real_tail = tail;
        let tail = if ht_on && crate::ht::config().lazy && ht_goal && resume {
            match Hints::unpack(hints.load()).huge_from {
                Some(huge) => {
                    let fr = region::floor(&host, region, self.epoch());
                    let g = layout.huge_pages();
                    let c = ((fr + g - 1) & !(g - 1)).max(huge);
                    if c < layout.region_pages() {
                        Some(tail.map_or(c, |t| t.min(c)))
                    } else {
                        tail
                    }
                }
                None => tail,
            }
        } else {
            tail
        };
        if ht_on {
            crate::ht::set_zone(region.index(), tail);
        }
        let mut ht_lowest = u32::MAX;
        if cursor != 0 && cursor <= floor {
            // Everything the cursor has left is below the frontier.""")
sub(P, """            let next = match layout.unit_below(carved, huge_from, cursor) {""",
    """            let next = match ht_unit_below(&layout, carved, huge_from, tail, cursor) {""")
sub(P, """                                let (outcome, _) = region::purge_unit(&host, region, run);
""", """                                let in_tail = tail.is_some_and(|t| run.start >= t);
                                let (huge, n) = (layout.huge_pages(), run.end - run.start);
                                let outcome = if in_tail
                                    && !crate::ht::may_return(region.index(), run.start, n, huge)
                                {
                                    Purge::Busy
                                } else {
                                    if in_tail
                                        && crate::ht::wants_split(region.index(), run.start, n, huge)
                                    {
                                        let t0 = crate::ht::cycles();
                                        host.advise(
                                            region,
                                            run.start as usize * page,
                                            n as usize * page,
                                            HugePages::Split,
                                        );
                                        crate::ht::timed(&crate::ht::SPLIT_CYC, Some(&crate::ht::SPLIT_MAX), t0);
                                        crate::ht::count(&crate::ht::SPLITS, 1);
                                    }
                                    let t0 = crate::ht::cycles();
                                    let (outcome, _) = region::purge_unit(&host, region, run);
                                    if in_tail {
                                        crate::ht::timed(&crate::ht::TAIL_PURGE_CYC, None, t0);
                                        if let Purge::Done { returned: bytes, .. } = outcome {
                                            let g2 = layout.huge_pages();
                                            let whole_unit = n == g2 && run.start % g2 == 0;
                                            if !whole_unit && ht_real_tail.is_none_or(|t| run.start < t) {
                                                ht_lowest = ht_lowest.min(run.start & !(g2 - 1));
                                            }
                                            crate::ht::count(&crate::ht::TAIL_PURGES, 1);
                                            crate::ht::count(&crate::ht::TAIL_PAGES, (bytes / page) as u64);
                                        }
                                    }
                                    outcome
                                };
""")
sub(P, """        let mut run = unit;
        if huge_from.is_some_and(|huge| unit.end > huge) {
            return (Some(run), read);
        }
        let layout = *host.layout();
        while let Step::Unit(next) = layout.unit_below(carved, huge_from, run.start) {
            if next.end != run.start || next.start < room.floor || run.end - next.start > room.pages
            {""", """        let mut run = unit;
        // Scratch (huge-tail): in the tail, runs are joined as in the small-page part, within
        // the tail and within one huge page, so that a split is asked for one unit at a time.
        let tail = crate::ht::zone_of(region.index()).filter(|_| crate::ht::config().on);
        let in_tail = tail.is_some_and(|t| unit.start >= t);
        if huge_from.is_some_and(|huge| unit.end > huge) && !in_tail {
            return (Some(run), read);
        }
        let layout = *host.layout();
        let floor = match tail {
            Some(t) if in_tail => room.floor.max(t).max((unit.end - 1) & !(layout.huge_pages() - 1)),
            _ => room.floor,
        };
        let below = if in_tail { None } else { huge_from };
        while let Step::Unit(next) = layout.unit_below(carved, below, run.start) {
            if next.end != run.start || next.start < floor || run.end - next.start > room.pages
            {""")
sub(P, """/// How much one step of the clocked pass may do""", """/// Scratch (huge-tail): `Layout::unit_below` for a region whose pages from `tail` up are
/// walked as small pages.
fn ht_unit_below(
    layout: &Layout,
    carved: u32,
    huge_from: Option<u32>,
    tail: Option<u32>,
    end: u32,
) -> Step {
    if let Some(t) = tail {
        let top = layout.carved_pages(carved);
        let e = if end == 0 || end > top { top } else { end };
        if e > t {
            return match layout.unit_below(carved, None, end) {
                Step::Unit(u) => Step::Unit(Unit {
                    start: if u.start < t { t } else { u.start },
                    end: u.end,
                }),
                other => other,
            };
        }
    }
    layout.unit_below(carved, huge_from, end)
}

/// How much one step of the clocked pass may do""")


# A unit of the tail that is wholly free goes back whole, in one call over its 2 MiB: that frees
# its huge page whole if it still has one, and its page table if not (CONFIG_PT_RECLAIM), so
# that once the advice is back on, a fault there is a huge page again (`z0` turns this off).
sub(P, """                        let (run, words) =
                            self.run_below(&host, region, unit, carved, huge_from, room);
                        read += words;""", """                        let g2 = layout.huge_pages();
                        let whole = Unit {
                            start: unit.end.saturating_sub(g2),
                            end: unit.end,
                        };
                        let (whole_ok, whole_read) = if crate::ht::config().whole
                            && unit.end % g2 == 0
                            && tail.is_some_and(|t| whole.start >= t)
                            && whole.start >= floor
                        {
                            region::purgeable_unit(&host, region, whole)
                        } else {
                            (false, 0)
                        };
                        read += whole_read;
                        let (run, words) = if whole_ok {
                            (Some(whole), 0)
                        } else {
                            self.run_below(&host, region, unit, carved, huge_from, room)
                        };
                        read += words;""")


# A split counts against the step's bound as `k` pages (`ht.rs`), so that a step that splits
# stops as one that returned that much would; and the cycles of each walk call in a region
# with a tail are timed (with `d1`).
sub(P, """        let mut first = true;
        // Where the last run this call purged began""", """        let mut first = true;
        let mut ht_spent = 0usize;
        let ht_t0 = if tail.is_some() { crate::ht::cycles() } else { 0 };
        // Where the last run this call purged began""")
sub(P, """            if !first && (read >= budget || (purged >= limit && !counting)) {""",
    """            if !first && (read >= budget || (purged + ht_spent >= limit && !counting)) {""")
sub(P, """                                        crate::ht::count(&crate::ht::SPLITS, 1);""",
    """                                        crate::ht::count(&crate::ht::SPLITS, 1);
                                        ht_spent += crate::ht::config().split_pages as usize * page;""")
sub(P, """        self.unretain(class, purged);
        if purged != 0 {""", """        // With `r2` the lowest unit returned is kept for the region until its walk reaches
        // the bottom (`done`), and the boundary comes down once a pass.
        if crate::ht::config().per_pass && ht_on {
            crate::ht::note_low(region.index(), ht_lowest);
            ht_lowest = if done { crate::ht::take_low(region.index()) } else { u32::MAX };
        }
        if ht_lowest != u32::MAX {
            let to = ht_real_tail.unwrap_or(layout.region_pages());
            if ht_lowest < to {
                let t0 = crate::ht::cycles();
                if host.advise(
                    region,
                    ht_lowest as usize * page,
                    (to - ht_lowest) as usize * page,
                    HugePages::Avoid,
                ) {
                    crate::ht::set_tail(region.index(), ht_lowest);
                }
                crate::ht::timed(&crate::ht::UNADVISE_CYC, Some(&crate::ht::UNADVISE_MAX), t0);
                if crate::ht::tracing() {
                    crate::ht::trace("L", crate::ht::mono_ns(), region.index() as u64, ht_lowest as u64, to as u64, 0, 0);
                }
            }
        }
        if tail.is_some() {
            crate::ht::timed(&crate::ht::STEP_CYC, Some(&crate::ht::STEP_MAX), ht_t0);
        }
        self.unretain(class, purged);
        if purged != 0 {""")


# Tracing (`HT_TRACE`): every walk call in a region with a huge-page part, every advice change
# and split the switch makes, with who ran it.
sub(P, """    pub fn thread_purge_steps(&self, now: &dyn Fn() -> u64) -> PassStep {""",
    """    pub fn thread_purge_steps(&self, now: &dyn Fn() -> u64) -> PassStep {
        crate::ht::note_bg();""")
sub(P, """        let mut first = true;
        let mut ht_spent = 0usize;""", """        let mut first = true;
        let ht_trace0 = if crate::ht::tracing() && Hints::unpack(hints.load()).huge_from.is_some() {
            crate::ht::mono_ns()
        } else {
            0
        };
        let ht_splits0 = crate::ht::SPLITS.load(core::sync::atomic::Ordering::Relaxed);
        let mut ht_spent = 0usize;""")
sub(P, """        if tail.is_some() {
            crate::ht::timed(&crate::ht::STEP_CYC, Some(&crate::ht::STEP_MAX), ht_t0);
        }""", """        if tail.is_some() {
            crate::ht::timed(&crate::ht::STEP_CYC, Some(&crate::ht::STEP_MAX), ht_t0);
        }
        if ht_trace0 != 0 {
            crate::ht::trace(
                "W",
                ht_trace0,
                region.index() as u64,
                (scope as u8 as u64) << 8 | u64::from(counting) << 1 | u64::from(tail.is_some()),
                read as u64,
                (purged / page) as u64,
                crate::ht::SPLITS.load(core::sync::atomic::Ordering::Relaxed) - ht_splits0,
            );
        }""")


sub(P, """    pub fn heartbeat_for(&self, now: &dyn Fn() -> u64, seen: Snapshot) -> bool {
        match seen.duty() {""", """    pub fn heartbeat_for(&self, now: &dyn Fn() -> u64, seen: Snapshot) -> bool {
        crate::ht::note_heartbeat();
        match seen.duty() {""")

# --- Heartbeats: the interval for the counters, and with `b1` the marked regions' advice.
sub(P, """            Duty::Thread => self.note_for_thread(now),
        }
    }""", """            // Scratch (huge-tail): a marked region is the thread's work.
            Duty::Thread => self.note_for_thread(now) | crate::ht::marked(),
        }
    }""")
sub(P, """    fn purge_steps_of(&self, now: &dyn Fn() -> u64, size: StepSize) -> PassStep {""",
    """    fn purge_steps_of(&self, now: &dyn Fn() -> u64, size: StepSize) -> PassStep {
        self.ht_beat();""")
sub(P, """    /// Takes the growth debt and purges toward it""", """    /// Scratch (huge-tail): the counters' interval, and with `b1` the advice of every region a
    /// claim marked put back on.
    fn ht_beat(&self) {
        let c = crate::ht::config();
        if !c.on {
            return;
        }
        if c.diag {
            crate::ht::note_epoch(self.epoch());
        }
        if !c.beat {
            return;
        }
        let plan = self.space.plan();
        let page = plan.geometry().page_size();
        let region_pages = (plan.region_size() / page) as u32;
        let epoch = self.epoch();
        let headroom = ((crate::ht::config().step_mib as usize) << 20) / page;
        crate::ht::beat(self.walk_regions(), region_pages, |index, from, to_end| {
            let region = plan.region(index)?;
            let RegionTag::Class(class) = self.state.table.get(region) else {
                return None;
            };
            let host = self.host(class)?;
            let layout = *host.layout();
            let g = layout.huge_pages();
            let fr = region::floor(&host, region, epoch);
            let to = if to_end {
                layout.region_pages()
            } else {
                ((fr + headroom as u32 + g - 1) & !(g - 1)).min(layout.region_pages())
            };
            if to <= from {
                return None;
            }
            let t0 = crate::ht::cycles();
            let _ = host.advise(region, from as usize * page, (to - from) as usize * page, HugePages::Prefer);
            crate::ht::timed(&crate::ht::READVISE_CYC, Some(&crate::ht::READVISE_MAX), t0);
            crate::ht::count(&crate::ht::READVISE, 1);
            if crate::ht::tracing() {
                crate::ht::trace("R", crate::ht::mono_ns(), index as u64, from as u64, to as u64, 0, 0);
            }
            crate::ht::forget_splits(index);
            Some(to)
        });
    }

    /// Takes the growth debt and purges toward it""")
