import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic, after `creditdiag.py`: how much of what large claims fault back a claim could have
# found resident in a free slot of the same class. A claim takes the lowest free slot of its
# class (`tree::claim`) whether its pages are resident or returned. At each large claim whose
# pages under the block were returned (R bytes), this scans every free slot of the class and
# finds the least any of them would have faulted back for the same block (M): `avoidable` adds
# R - M, and `avoidable_whole` adds R when some free slot was resident under the whole block.
# The scan is slow; the build is for counting, not for timing.
D = "crate::os::linux::creditdiag"
R = "crates/asmalloc/src/back/region.rs"
sub(R, """/// `settle` for one large slot, just claimed""", """/// DIAGNOSTIC: the bytes under a block of `size` bytes in large slot `slot` whose pages are clean
/// (returned or never written): what a claim of it for that block faults back.
pub(crate) fn refault_of<H: Host>(host: &H, region: RegionId, slot: u32, size: usize) -> usize {
    let layout = host.layout();
    let Some(unit) = layout.slot_unit(slot) else {
        return 0;
    };
    let end = extent_of(layout, slot, unit, size, false);
    if end <= unit.start {
        return 0;
    }
    let mut clean = 0usize;
    for (w, mask) in page_words(unit.start, end) {
        clean += count(host.meta(region, layout.clean_word(w)).load() & mask);
    }
    clean * layout.page_size()
}

/// `settle` for one large slot, just claimed""")

C = "crates/asmalloc/src/back/class.rs"
sub(C, """            let settled = if small {
                region::settle(&host, taken.region, taken.word, taken.bits)
            } else {
                region::settle_slot(&host, taken.region, slot, size)
            };""", f"""            if !small {{
                {D}::add(&{D}::CLAIMS_LARGE, 1);
                let r = region::refault_of(&host, taken.region, slot, size);
                if r != 0 {{
                    {D}::add(&{D}::CLAIMS_LARGE_REFAULT, 1);
                    self.diag_scan(&host, class, size, r);
                }}
            }}
            let settled = if small {{
                region::settle(&host, taken.region, taken.word, taken.bits)
            }} else {{
                region::settle_slot(&host, taken.region, slot, size)
            }};""")
sub(C, """    fn alloc_at(&self, class: Class, size: usize, stagger: bool) -> Option<Block> {""", f"""    /// DIAGNOSTIC: the least any free slot of `class` would fault back for a block of `size`,
    /// against `r` that the claim made faults back.
    #[cold]
    #[inline(never)]
    fn diag_scan(&self, host: &ClassHost<'a, O>, class: Class, size: usize, r: usize) {{
        {D}::add(&{D}::SCANS, 1);
        let mut least = usize::MAX;
        let plan = self.space.plan();
        'regions: for i in 0..self.walk_regions() {{
            let Some(region) = plan.region(i) else {{ continue }};
            if self.state.table.get(region) != RegionTag::Class(class) {{
                continue;
            }}
            let carved = Grow::unpack(host.grow(region).load()).carved.min(host.layout().slots());
            if carved == 0 {{
                continue;
            }}
            for w in 0..=(carved - 1) / BITS {{
                let mut bits = tree::word(host, tree::Node::leaf(region, w)).load()
                    & super::word::bits_in_word(w, 0, carved - 1);
                while bits != 0 {{
                    let slot = w * BITS + bits.trailing_zeros();
                    bits &= bits - 1;
                    {D}::add(&{D}::SCAN_SLOTS, 1);
                    least = least.min(region::refault_of(host, region, slot, size));
                    if least == 0 {{
                        break 'regions;
                    }}
                }}
            }}
        }}
        if least != usize::MAX {{
            {D}::add(&{D}::AVOIDABLE, r.saturating_sub(least));
            if least == 0 {{
                {D}::add(&{D}::AVOIDABLE_WHOLE, r);
            }}
        }}
    }}

    fn alloc_at(&self, class: Class, size: usize, stagger: bool) -> Option<Block> {{""")
