//! The address space: where regions, their metadata and the magazines live.
//!
//! At first use the allocator reserves one range of address space: up to 16 TiB in the design's
//! [`Geometry`], less if the process is not allowed that much. Most of it is object regions of
//! 4 GiB, each assigned to one class on demand and holding slots of that class at
//! `region_base + index * class_size`. Above the object regions sit one metadata block per region
//! and the magazine area ([`Plan`]). Memory that holds objects never holds allocator state.
//!
//! So every heap pointer has two properties without per-object metadata: its region is
//! `(ptr - base) >> region_shift`, and its slot index is `offset / class_size`, computed without
//! a divide ([`Location`]). The [`RegionTable`] gives each region's class.
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
pub use plan::{Location, Plan, RegionId};
pub use reservation::{Space, SpaceCell};
pub use table::{RegionTable, RegionTag};
