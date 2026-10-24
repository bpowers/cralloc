#!/usr/bin/env python3
"""The run-time switch for docs/research/whole-block-huge.md, for measurement only. Applied to a
scratch copy of the repository (build.sh), it puts wbh.rs into the allocator as `crate::wbh`
and gives a large block allocated whole (a plain allocation, not the destination of a growth
with headroom) the huge-page mark that the grown path gives a block that grows densely: the
slot's whole huge pages up to the block's size advised for huge pages and `Recorded::huge` set
while it holds the slot; its free returns the slot whole and takes the advice back, or keeps it
within the kept-grown budget, exactly as the free of a grown block does. With `ASMALLOC_WBH`'s
`d`, it also records each assigned region's base and class for the map at exit.

  wbh_edit.py TREE
"""
import shutil
import sys
from pathlib import Path

TREE = Path(sys.argv[1])
HERE = Path(__file__).resolve().parent
SRC = TREE / "crates/asmalloc/src"


def sub(path, old, new, count=1):
    p = SRC / path
    s = p.read_text()
    if s.count(old) != count:
        raise SystemExit(f"{path}: {s.count(old)} matches (want {count}) for {old[:90]!r}")
    p.write_text(s.replace(old, new))


shutil.copy(HERE / "wbh.rs", SRC / "wbh.rs")
sub("lib.rs", "mod heap;\n", "mod heap;\npub mod wbh;\n")

# --- Heap: a plain large allocation is "whole"; a growth's destination with room is not.
L = "heap/large.rs"
sub(L, """    pub(crate) fn alloc_large(&self, layout: Layout, zero: Zero) -> Option<Grant> {
        self.alloc_large_using(layout, layout.size(), zero)
    }""", """    pub(crate) fn alloc_large(&self, layout: Layout, zero: Zero) -> Option<Grant> {
        self.wbh_alloc_large(layout, layout.size(), zero, true)
    }""")
sub(L, """    pub(crate) fn alloc_large_using(
        &self,
        layout: Layout,
        used: usize,
        zero: Zero,
    ) -> Option<Grant> {
        let back = self.start()?;""", """    pub(crate) fn alloc_large_using(
        &self,
        layout: Layout,
        used: usize,
        zero: Zero,
    ) -> Option<Grant> {
        self.wbh_alloc_large(layout, used, zero, false)
    }

    /// Scratch (whole-block-huge): `alloc_large_using`, told whether the block is allocated
    /// whole.
    fn wbh_alloc_large(&self, layout: Layout, used: usize, zero: Zero, whole: bool) -> Option<Grant> {
        let back = self.start()?;""")
sub(L, """                let len = block.len;
                let clear = match zero {""", """                let len = block.len;
                if whole && used >= back.space().plan().geometry().huge_page_size() {
                    crate::wbh::count_whole(used, zero != Zero::No);
                    // Before any clearing, so that a `memset` already faults huge pages.
                    if crate::wbh::wants(used, zero != Zero::No) {
                        back.wbh_advise(block.ptr, used);
                    }
                }
                let clear = match zero {""")
sub(L, """    pub(crate) fn alloc_large_slow(self, layout: Layout, zero: Zero) -> Option<Grant> {
        self.alloc_large_slow_using(layout, layout.size(), zero)
    }""", """    pub(crate) fn alloc_large_slow(self, layout: Layout, zero: Zero) -> Option<Grant> {
        let grant = self.heap().alloc_large(layout, zero);
        self.rust_heartbeat();
        grant
    }""")

# --- Back: the advice, the frees of advised slots counted, the map of regions.
C = "back/class.rs"
sub(C, """        if self.keep_grown(class, charge) {
            return (0, true);
        }
        // SAFETY: BACK-CLASS-01 in docs/safety/asmalloc/back/class-safety.md. The program is
        // freeing the slot and has not been given it back, its bit was clear a moment ago,
        // and the slot is below the watermark (`slot_in`).
        let purged = unsafe { region::purge_held(host, region, slot) };
        region::unadvise(host, region, slot);""", """        if self.keep_grown(class, charge) {
            crate::wbh::add(crate::wbh::FREE_KEPT_N, 1);
            return (0, true);
        }
        // SAFETY: BACK-CLASS-01 in docs/safety/asmalloc/back/class-safety.md. The program is
        // freeing the slot and has not been given it back, its bit was clear a moment ago,
        // and the slot is below the watermark (`slot_in`).
        let purged = unsafe { region::purge_held(host, region, slot) };
        if purged {
            crate::wbh::add(crate::wbh::FREE_RET_N, 1);
            crate::wbh::add(crate::wbh::FREE_RET_B, charge);
        }
        region::unadvise(host, region, slot);""")
sub(C, """            let published = region.index() as u32 + 1;
            if host
                .class
                .carving
                .compare_exchange(seen, published, SeqCst, SeqCst)
                .is_ok()
            {
                return true;
            }""", """            let published = region.index() as u32 + 1;
            if host
                .class
                .carving
                .compare_exchange(seen, published, SeqCst, SeqCst)
                .is_ok()
            {
                crate::wbh::region(
                    region.index(),
                    self.space.region(region).addr().get(),
                    plan.region_size(),
                    class.size(),
                    class.is_small(),
                );
                return true;
            }""")

HELPERS = r'''
/// Scratch (bench/probes/whole-block-huge/wbh_edit.py). Not product code.
impl<'a, O: Os> Back<'a, O> {
    /// For the caller that has just been given the large block at `ptr` for `used` bytes,
    /// allocated whole: advises its slot's whole huge pages up to `used` for huge pages and
    /// records `huge`, as `region::advise_grown` does for a block that grows densely, so that
    /// its free takes the grown path (`purge_before_release`).
    pub(crate) fn wbh_advise(&self, ptr: NonNull<u8>, used: usize) {
        let Some(Kind::Slot(class)) = self.lookup(ptr.as_ptr()) else {
            return;
        };
        if class.is_small() || class.size() < self.space.plan().geometry().huge_page_size() {
            return;
        }
        let Some(host) = self.host(class) else {
            return;
        };
        let (region, slot) = self.validate(&host, ptr.as_ptr(), class);
        let into = self.offset_in_slot(region, class, slot, ptr.as_ptr());
        let layout = host.layout();
        let Some(now) = region::recorded(&host, region, slot) else {
            return;
        };
        let Some(unit) = layout.huge_interior(slot) else {
            crate::wbh::add(crate::wbh::NOFIT_N, 1);
            return;
        };
        let page = layout.page_size();
        // Rounded down, as the grown path does: the huge page the block only starts stays on
        // small pages.
        let wanted =
            ((layout.slot_bytes(slot) + into + used) / page) as u32 & !(layout.huge_pages() - 1);
        let end = wanted.min(unit.end);
        if end <= unit.start {
            crate::wbh::add(crate::wbh::NOFIT_N, 1);
            return;
        }
        region::record(&host, region, slot, Recorded { huge: true, ..now });
        let (offset, len) = (
            unit.start as usize * page,
            (end - unit.start) as usize * page,
        );
        if host.advise(region, offset, len, HugePages::Prefer) {
            crate::wbh::add(crate::wbh::ADV_N, 1);
            crate::wbh::add(crate::wbh::ADV_B, len);
        } else {
            crate::wbh::add(crate::wbh::ADV_FAIL, 1);
        }
        region::cover(&host, region, slot);
    }
}
'''
p = SRC / C
p.write_text(p.read_text() + HELPERS)
print("whole-block-huge switch applied to", TREE)
