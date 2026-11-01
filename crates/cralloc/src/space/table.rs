//! The class-of-region table: one byte per region saying what the region holds.
//!
//! The C entry points and large objects find a pointer's class here: one load, at
//! `(ptr - base) >> region_shift`, from a 4096-byte table that fits in the L1 cache. A zero byte
//! means unassigned, so an all-zero table is a valid table for a heap with nothing in it.

use super::{MAX_REGIONS, RegionId};
use crate::cells::Cells;
use crate::classes::{COUNT, Class};
use core::sync::atomic::{AtomicU8, Ordering};

/// What a region holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegionTag {
    /// Nothing: never assigned, or a region beyond the object regions.
    Unassigned,
    /// Slots of one class, at `region_base + origin + index * class_size` (`Plan::slot_origin`).
    Class(Class),
    /// The first region of an object larger than a region; it starts at the region's base.
    RunHead,
    /// A later region of such an object. No object starts here.
    RunTail,
}

const UNASSIGNED: u8 = 0;
const RUN_HEAD: u8 = 0xfe;
const RUN_TAIL: u8 = 0xff;
const _: () = assert!(COUNT < RUN_HEAD as usize);

impl RegionTag {
    /// The table byte: 0 for unassigned, the class index for a class, and two values above the
    /// classes for runs.
    #[inline(always)]
    pub const fn to_byte(self) -> u8 {
        match self {
            RegionTag::Unassigned => UNASSIGNED,
            RegionTag::Class(class) => class.index(),
            RegionTag::RunHead => RUN_HEAD,
            RegionTag::RunTail => RUN_TAIL,
        }
    }

    /// The tag for a table byte. The table only holds bytes `to_byte` made, so the bytes that
    /// are no tag never occur; they read as unassigned, so a pointer there is refused.
    #[inline(always)]
    pub const fn from_byte(byte: u8) -> RegionTag {
        match byte {
            RUN_HEAD => RegionTag::RunHead,
            RUN_TAIL => RegionTag::RunTail,
            _ => match Class::from_index(byte) {
                Some(class) => RegionTag::Class(class),
                None => RegionTag::Unassigned,
            },
        }
    }
}

/// The class-of-region table.
///
/// Entries change only by compare-and-swap, so two threads cannot both assign a region.
pub struct RegionTable {
    tags: Cells<AtomicU8, MAX_REGIONS>,
}

impl RegionTable {
    /// A table with every region unassigned.
    pub const fn new() -> RegionTable {
        RegionTable {
            tags: Cells::new([const { AtomicU8::new(UNASSIGNED) }; MAX_REGIONS]),
        }
    }

    fn entry(&self, region: RegionId) -> &AtomicU8 {
        // `RegionId` is below MAX_REGIONS, so the table's reduction modulo its length changes
        // nothing, and there is no bounds check to panic.
        self.tags.get(region.index())
    }

    /// What a region holds.
    ///
    /// Acquire, so that whoever learns a pointer's class from the table also sees everything the
    /// assigning thread wrote before assigning.
    #[inline(always)]
    pub fn get(&self, region: RegionId) -> RegionTag {
        RegionTag::from_byte(self.entry(region).load(Ordering::Acquire))
    }

    /// Changes a region's tag from `current` to `new` in one compare-and-swap. On failure returns
    /// the tag the region had instead.
    #[inline]
    pub fn replace(
        &self,
        region: RegionId,
        current: RegionTag,
        new: RegionTag,
    ) -> Result<(), RegionTag> {
        self.entry(region)
            .compare_exchange(
                current.to_byte(),
                new.to_byte(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(RegionTag::from_byte)
    }
}

impl Default for RegionTable {
    fn default() -> RegionTable {
        RegionTable::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tag_round_trips_and_other_bytes_read_as_unassigned() {
        for byte in 0..=u8::MAX {
            let tag = RegionTag::from_byte(byte);
            if tag == RegionTag::Unassigned {
                assert!(byte == 0 || (COUNT < byte as usize && byte < RUN_HEAD));
            } else {
                assert_eq!(tag.to_byte(), byte);
            }
        }
        for class in Class::all() {
            assert_eq!(
                RegionTag::from_byte(RegionTag::Class(class).to_byte()),
                RegionTag::Class(class)
            );
        }
    }

    #[test]
    fn a_region_is_assigned_once() {
        let table = RegionTable::new();
        let region = RegionId::new(7).unwrap();
        let class = RegionTag::Class(Class::SMALLEST);
        assert_eq!(table.get(region), RegionTag::Unassigned);
        assert_eq!(table.replace(region, RegionTag::Unassigned, class), Ok(()));
        assert_eq!(
            table.replace(region, RegionTag::Unassigned, RegionTag::RunHead),
            Err(class)
        );
        assert_eq!(table.get(region), class);
        assert_eq!(
            table.get(RegionId::new(MAX_REGIONS - 1).unwrap()),
            RegionTag::Unassigned
        );
    }
}
