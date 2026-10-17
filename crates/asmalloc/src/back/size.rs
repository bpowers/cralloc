//! The size each large object was last asked for, kept out of band.
//!
//! A large object may sit in a slot or run far bigger than it is (a resize grows it into a
//! block with headroom), so the block's size says little about the object's. Its owner records
//! the size it asked for whenever it allocates or resizes in place: one word per slot in the
//! region's metadata (`Layout::size_word`), or one per region in the state for a run, by its
//! head. Only the owner of the block writes or reads the word, so a plain store and load do;
//! a word left behind by a freed block is overwritten by the next owner before it reads it.
//! Small objects record nothing: their size is their class.

use super::tree::Host;
use super::word::Word;
use super::{Back, Kind};
use crate::os::Os;
use crate::space::RegionTag;
use core::ptr::NonNull;
use core::sync::atomic::Ordering::Relaxed;

impl<O: Os> Back<'_, O> {
    /// Records that the large block starting at `ptr`, which the caller owns, now holds
    /// `size` bytes. Does nothing for a small slot or a pointer that starts no block.
    pub fn record_size(&self, ptr: NonNull<u8>, size: usize) {
        let Some(location) = self.space.locate(ptr.as_ptr()) else {
            return;
        };
        match self.state.table.get(location.region) {
            RegionTag::Class(class) if !class.is_small() => {
                let Some(host) = self.host(class) else { return };
                let Some(slot) = self.space.plan().slot_at(location, class) else {
                    return;
                };
                if let Some(word) = host.layout().size_word(slot) {
                    host.meta(location.region, word).store(size as u64);
                }
            }
            RegionTag::RunHead if location.offset == 0 => {
                self.run_size(location.region.index()).store(size, Relaxed);
            }
            _ => {}
        }
    }

    /// The size last recorded for the large block starting at `ptr`, which the caller owns.
    /// `None` for a small slot or a pointer that starts no block.
    pub fn recorded_size(&self, ptr: *const u8) -> Option<usize> {
        let location = self.space.locate(ptr)?;
        match self.lookup(ptr)? {
            Kind::Slot(class) if !class.is_small() => {
                let host = self.host(class)?;
                let slot = self.space.plan().slot_at(location, class)?;
                let word = host.layout().size_word(slot)?;
                Some(host.meta(location.region, word).load() as usize)
            }
            Kind::Run(_) => Some(self.run_size(location.region.index()).load(Relaxed)),
            Kind::Slot(_) => None,
        }
    }

    /// The recorded size of the run whose head is region `index`.
    fn run_size(&self, index: usize) -> &core::sync::atomic::AtomicUsize {
        &self.state.run_sizes[index % self.state.run_sizes.len()]
    }
}
