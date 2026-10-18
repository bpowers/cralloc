//! The back end's part of the introspection walk (`crate::stats`): regions, how far each is
//! carved and committed, and the runs of free slots in their bitmaps.
//!
//! Read-only and racy. It reads only metadata words the protocols may read: the fixed part of a
//! region's block, committed before the region carries its class tag, and leaf words covering
//! slots below a watermark it has just read, committed before that watermark was published.

use super::Back;
use super::layout::{Grow, Hints};
use super::tree::Host;
use super::word::{BITS, Word};
use crate::classes::Class;
use crate::os::Os;
use crate::space::{RegionId, RegionTag};
use crate::stats::{Region, Visitor};

impl<O: Os> Back<'_, O> {
    /// Reports the retained count, every assigned region and every run of free slots.
    pub fn inspect(&self, visitor: &mut impl Visitor) {
        visitor.retained(self.retained());
        let plan = self.space.plan();
        for region in plan.regions() {
            match self.state.table.get(region) {
                RegionTag::Class(class) => self.inspect_region(class, region, visitor),
                RegionTag::RunHead => {
                    let base = self.space.region(region).as_ptr().cast_const();
                    let len = self.run_length(region) << plan.region_shift();
                    visitor.run(base, len, self.run_size(base));
                }
                _ => {}
            }
        }
    }

    fn inspect_region(&self, class: Class, region: RegionId, visitor: &mut impl Visitor) {
        let Some(host) = self.host(class) else {
            return;
        };
        let layout = *host.layout();
        let page = layout.page_size();
        let grow = Grow::unpack(host.grow(region).load());
        let hints = Hints::unpack(host.hints(region).load());
        let carved = grow.carved.min(layout.slots());
        let base = self.space.region(region).as_ptr().cast_const();
        visitor.region(&Region {
            class,
            index: region.index(),
            base,
            len: self.space.plan().region_size(),
            carved: layout.slot_bytes(carved),
            committed: grow.committed as usize * page,
            huge_from: hints.huge_from.map(|from| from as usize * page),
            metadata: self.space.metadata(region).as_ptr().cast_const(),
            metadata_len: layout.block_bytes(),
        });
        // Runs of consecutive set bits, joined across words.
        let mut run: Option<(u32, u32)> = None;
        let mut emit = |first: u32, end: u32| {
            let start = base.wrapping_add(layout.slot_bytes(first));
            visitor.free(class, start, layout.slot_bytes(end - first));
        };
        for w in 0..carved.div_ceil(BITS) {
            let mut bits = host.meta(region, layout.tree_word(0, w)).load();
            while bits != 0 {
                let low = bits.trailing_zeros();
                let length = (!(bits >> low)).trailing_zeros();
                bits &= !super::word::span(low, low + length - 1);
                let (first, end) = (w * BITS + low, w * BITS + low + length);
                run = match run {
                    Some((start, at)) if at == first => Some((start, end)),
                    Some((start, at)) => {
                        emit(start, at);
                        Some((first, end))
                    }
                    None => Some((first, end)),
                };
            }
        }
        if let Some((start, at)) = run {
            emit(start, at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::State;
    use super::*;
    use crate::classes::SmallClass;
    use crate::os::sim::{SimOs, Standard};
    use std::boxed::Box;
    use std::vec::Vec;

    #[derive(Default)]
    struct Seen {
        regions: Vec<(Class, usize, usize)>,
        free: Vec<(usize, usize)>,
        retained: usize,
    }

    impl Visitor for Seen {
        fn retained(&mut self, bytes: usize) {
            self.retained = bytes;
        }
        fn region(&mut self, region: &Region) {
            self.regions
                .push((region.class, region.carved, region.committed));
        }
        fn free(&mut self, _: Class, start: *const u8, len: usize) {
            self.free.push((start.addr(), len));
        }
    }

    #[test]
    fn the_walk_reports_carved_regions_and_joined_free_runs() {
        let state = Box::new(State::new(SimOs::<Standard>::new()));
        let back = state.get_or_reserve().unwrap();
        let class = SmallClass::from_ordinal(1).unwrap();
        let size = class.size();
        let mut objects = [core::ptr::null_mut(); 200];
        assert_eq!(back.fill(class, &mut objects[..]), 200);
        let base = objects[0].addr();
        // Free slots 10..150 (a run that crosses word boundaries) and slot 199.
        let mut freed: Vec<*mut u8> = objects[10..150].to_vec();
        freed.push(objects[199]);
        // SAFETY: the test holds these slots, or passes a bad one to see the abort.
        unsafe { back.launder(class, &freed) };
        let mut seen = Seen::default();
        back.inspect(&mut seen);
        assert_eq!(seen.regions.len(), 1);
        let (seen_class, carved, committed) = seen.regions[0];
        assert_eq!(seen_class, class.class());
        assert_eq!(carved, 200 * size);
        assert!(committed >= carved);
        assert_eq!(
            seen.free,
            [(base + 10 * size, 140 * size), (base + 199 * size, size)]
        );
        assert_eq!(seen.retained, 141 * size);
    }
}
