//! The address space: where regions, their metadata and the magazines live.
//!
//! At first use the allocator reserves one range of address space: 16 TiB in the design's
//! [`Geometry`]. Most of it is 3940 object regions of 4 GiB, each assigned to one class on demand
//! and holding slots of that class at `region_base + origin + index * class_size`, the origin
//! a few whole pages for a small class and zero for a large one (`Plan::slot_origin`). Above the object
//! regions sit the magazine area, one metadata block per region and the process page ([`Plan`]).
//! Memory that holds objects never holds allocator state. If the process cannot have that much
//! address space, the region size is halved, keeping the number of regions, down to the
//! geometry's floor; the size a reservation got is its plan's region shift.
//!
//! So every heap pointer has two properties without per-object metadata: its region is
//! `(ptr - base) >> region_shift`, and its slot index is `offset / class_size`, computed without
//! a divide ([`Location`]). The [`RegionTable`] gives each region's class. The region shift
//! comes from the published [`Space`], not from a constant.
//!
//! The geometry is a parameter, fixed by the [`crate::os::Os`] in use, so the simulated OS can
//! host a heap of a few megabytes. Everything here is shifts, masks and adds; `verify.rs` proves
//! the round trips and bounds with Kani.

mod geometry;
mod plan;
mod reservation;
mod table;
#[cfg(kani)]
mod verify;
#[cfg(kani)]
mod verify_slow;

pub use geometry::{Geometry, MAX_REGIONS, METADATA_SHIFT};
pub(crate) use plan::slot_from;
pub use plan::{Location, ORIGIN_SHARE, Plan, RegionId};
pub use reservation::{Space, SpaceCell};
pub use table::{RegionTable, RegionTag};
