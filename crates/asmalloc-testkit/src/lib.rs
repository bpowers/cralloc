//! The test kit that judges asmalloc.
//!
//! It drives an allocator through streams of operations and checks every result against a
//! shadow model of the live blocks: alignment, no overlap with any live block, usable size,
//! zero fill, contents carried over by resizes, and that the allocator never writes into live
//! memory. The same model runs against `GlobalAlloc`, `Allocator` and the C functions, on one
//! thread or many, from a seed or from a fuzzer's bytes.
//!
//! - [`subject`]: the interfaces under test behind one trait, [`subject::Subject`].
//! - [`profile`] and [`generator`]: the shapes of operation streams, and the code that draws
//!   operations from entropy.
//! - [`model`]: the checks, one operation at a time; [`shadow`] is the shared index of live
//!   blocks that the overlap check uses.
//! - [`driver`]: single- and multithreaded runs, hand-off between threads, thread exit, fork,
//!   replay of a recorded script, and minimization.
//! - [`mutants`]: allocators broken on purpose, to show the model catches each kind of bug.
//! - [`conformance`]: edge cases of the C functions, as data.
//! - [`target`]: the allocator the suite judges.

// A `Failure` is a few hundred bytes and travels only on the path that ends a run, so boxing it
// would add noise to every signature for no gain.
#![allow(clippy::result_large_err)]

pub mod conformance;
pub mod driver;
pub mod entropy;
pub mod failure;
pub mod generator;
pub mod mail;
pub mod model;
pub mod mutants;
pub mod op;
pub mod pattern;
pub mod profile;
pub mod shadow;
pub mod subject;
pub mod target;

#[cfg(all(target_os = "linux", not(miri)))]
mod sys;

pub use driver::{
    Config, ForkPlan, Handoff, check, check_minimized, generate, minimize, replay, run, run_bytes,
};
pub use failure::{Failure, FailureKind, Trace};
pub use model::Stats;
pub use op::{BlockId, Op, Script};
pub use profile::Profile;
