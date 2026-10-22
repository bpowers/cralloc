import os
import sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sub import sub

# Diagnostic, after `credit2.py`: a large block that grows in place over pages that were
# returned or never written (`region::cover` clears their clean bits) makes them resident as a
# carve would, and is charged to the growth debt. Main charges only carves, and `credit2.py`
# adds claims; on DataFusion, whose buffers grow by `realloc`, growth in place went unpaid while
# hundreds of MiB of free large memory stayed resident.
R = "crates/asmalloc/src/back/region.rs"
sub(R, """pub(crate) fn cover<H: Host>(host: &H, region: RegionId, slot: u32) {
    let layout = host.layout();
    let (Some(unit), Some(now)) = (layout.slot_unit(slot), recorded(host, region, slot)) else {
        return;
    };
    let end = extent_of(layout, slot, unit, now.size, now.huge);
    if end <= unit.start {
        return;
    }
    for (w, inside) in page_words(unit.start, end) {
        let word = host.meta(region, layout.clean_word(w));
        if word.load() & inside != 0 {
            word.fetch_and(!inside);
        }
    }
}""", """pub(crate) fn cover<H: Host>(host: &H, region: RegionId, slot: u32) -> usize {
    let layout = host.layout();
    let (Some(unit), Some(now)) = (layout.slot_unit(slot), recorded(host, region, slot)) else {
        return 0;
    };
    let end = extent_of(layout, slot, unit, now.size, now.huge);
    if end <= unit.start {
        return 0;
    }
    let mut cleared = 0;
    for (w, inside) in page_words(unit.start, end) {
        let word = host.meta(region, layout.clean_word(w));
        if word.load() & inside != 0 {
            cleared += count(word.fetch_and(!inside) & inside);
        }
    }
    cleared * layout.page_size()
}""")
C = "crates/asmalloc/src/back/class.rs"
s = open(C).read()
n = s.count("region::cover(&host, region, slot);")
assert n == 3, n
s = s.replace("region::cover(&host, region, slot);", "self.grow(region::cover(&host, region, slot));")
open(C, "w").write(s)
sub(C, "region::cover(&host, region, index);", "self.grow(region::cover(&host, region, index));")
