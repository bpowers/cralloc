//! What the model caught, with what it takes to reproduce it.

use core::fmt;

use crate::driver::Config;
use crate::op::{Op, Script};

/// The violated property.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailureKind {
    /// A block was not aligned as the request and the interface require.
    Misaligned,
    /// A block's usable size was below the request.
    ShortBlock,
    /// A block overlapped a live block (or wrapped around the address space).
    Overlap,
    /// Memory promised to be zero was not.
    NotZeroed,
    /// A live block no longer held what the model wrote: something wrote into it.
    Corrupted,
    /// A resize did not carry over the bytes it promised to.
    LostContents,
    /// The usable size reported for a live block changed.
    UsableSizeChanged,
    /// The allocator refused a request inside the live-byte budget.
    Refused,
    /// The child of a `fork` died from a signal.
    ChildCrashed,
    /// The child of a `fork` did not finish in time (a deadlock, most likely).
    ChildHung,
    /// The child of a `fork` panicked.
    ChildPanicked,
}

impl FailureKind {
    /// Every kind.
    pub const ALL: [FailureKind; 11] = [
        FailureKind::Misaligned,
        FailureKind::ShortBlock,
        FailureKind::Overlap,
        FailureKind::NotZeroed,
        FailureKind::Corrupted,
        FailureKind::LostContents,
        FailureKind::UsableSizeChanged,
        FailureKind::Refused,
        FailureKind::ChildCrashed,
        FailureKind::ChildHung,
        FailureKind::ChildPanicked,
    ];

    /// The kind whose `Debug` name is `name`.
    pub fn from_name(name: &str) -> Option<FailureKind> {
        FailureKind::ALL
            .into_iter()
            .find(|k| format!("{k:?}") == name)
    }
}

/// The operations leading to a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trace {
    /// The whole stream of a single-worker run, up to and including the failing operation.
    /// It replays exactly with [`crate::driver::replay`] and shrinks with
    /// [`crate::driver::minimize`].
    Complete(Script),
    /// The last operations of the failing worker of a multithreaded run, for diagnosis.
    Recent(Script),
}

/// A contract violation.
#[derive(Clone, Debug)]
pub struct Failure {
    /// The violated property.
    pub kind: FailureKind,
    /// The interface under test.
    pub subject: String,
    /// Index of the worker that noticed.
    pub worker: usize,
    /// Index of the operation in that worker's stream. A [`FailureKind::Corrupted`] block was
    /// damaged by some earlier operation, possibly another worker's; this is when the model
    /// looked.
    pub op_index: usize,
    /// The operation, when the failure arose in one.
    pub op: Option<Op>,
    /// Addresses, sizes and bytes involved.
    pub detail: String,
    /// The configuration of a seeded run, for rerunning it.
    pub origin: Option<Config>,
    /// The operations that led here.
    pub trace: Option<Trace>,
}

/// Scripts up to this many operations are printed in full.
const PRINT_LIMIT: usize = 60;

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} in {}: {}", self.kind, self.subject, self.detail)?;
        write!(f, "\n  at op {} of worker {}", self.op_index, self.worker)?;
        if let Some(op) = &self.op {
            write!(f, " ({op})")?;
        }
        if let Some(c) = &self.origin {
            write!(
                f,
                "\n  rerun: profile {}, seed {:#x}, {} ops on each of {} workers, hand-off {:?}",
                c.profile.name, c.seed, c.ops, c.workers, c.handoff
            )?;
            if c.workers > 1 {
                write!(f, " (thread interleavings do not replay exactly)")?;
            }
        }
        match &self.trace {
            Some(Trace::Complete(s)) if s.0.len() <= PRINT_LIMIT => {
                write!(f, "\n  replay: \"{s}\"")?;
            }
            Some(Trace::Complete(s)) => write!(
                f,
                "\n  replay: a script of {} ops is in Failure::trace; driver::minimize shrinks it",
                s.0.len()
            )?,
            Some(Trace::Recent(s)) => {
                write!(
                    f,
                    "\n  last {} ops of worker {}: \"{s}\"",
                    s.0.len(),
                    self.worker
                )?;
            }
            None => {}
        }
        Ok(())
    }
}

impl std::error::Error for Failure {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::BlockId;

    #[test]
    fn kinds_round_trip_by_name() {
        for k in FailureKind::ALL {
            assert_eq!(FailureKind::from_name(&format!("{k:?}")), Some(k));
        }
        assert_eq!(FailureKind::from_name("Nope"), None);
    }

    #[test]
    fn a_short_trace_prints_as_a_replayable_script() {
        let script = Script(vec![
            Op::Alloc {
                id: BlockId(1),
                size: 8,
                align: 8,
                zeroed: false,
                variant: 0,
            },
            Op::Free {
                id: BlockId(1),
                variant: 0,
            },
        ]);
        let f = Failure {
            kind: FailureKind::Overlap,
            subject: "s".into(),
            worker: 0,
            op_index: 1,
            op: script.0.last().copied(),
            detail: "d".into(),
            origin: None,
            trace: Some(Trace::Complete(script.clone())),
        };
        let text = f.to_string();
        assert!(
            text.contains("replay: \"alloc 1 8 8 0; free 1 0\""),
            "{text}"
        );
        let quoted = text.split('"').nth(1).unwrap();
        assert_eq!(quoted.parse::<Script>(), Ok(script));
    }
}
