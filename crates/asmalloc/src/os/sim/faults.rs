//! Fault injection: which simulated calls fail, and when.

use crate::os::OsError;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A simulated OS call that can fail.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Call {
    /// [`crate::os::Os::reserve`].
    Reserve,
    /// [`crate::os::Os::commit`].
    Commit,
    /// [`crate::os::Os::purge`].
    Purge,
    /// [`crate::os::Os::advise_huge`].
    AdviseHuge,
    /// [`crate::os::Os::resident`].
    Resident,
    /// [`crate::os::Os::wipe_on_fork`].
    WipeOnFork,
    /// [`crate::os::Os::unmap`].
    Unmap,
    /// [`crate::os::Os::register_fence`].
    RegisterFence,
    /// [`crate::os::Os::fence`].
    Fence,
    /// [`crate::os::Os::split_huge`].
    SplitHuge,
}

impl Call {
    /// Every call.
    pub const ALL: [Call; 10] = [
        Call::Reserve,
        Call::Commit,
        Call::Purge,
        Call::AdviseHuge,
        Call::Resident,
        Call::WipeOnFork,
        Call::Unmap,
        Call::RegisterFence,
        Call::Fence,
        Call::SplitHuge,
    ];

    /// The error an injected failure reports: what Linux reports for the usual cause.
    pub const fn error(self) -> OsError {
        match self {
            // Out of address space or commit charge, or too many mappings.
            Call::Reserve
            | Call::Commit
            | Call::Purge
            | Call::AdviseHuge
            | Call::Unmap
            | Call::SplitHuge => OsError::ENOMEM,
            // A range that is not mapped.
            Call::Resident => OsError::ENOMEM,
            // A kernel before 4.14.
            Call::WipeOnFork => OsError::EINVAL,
            // A kernel without rseq membarrier support.
            Call::RegisterFence => OsError::EINVAL,
            // Fencing without registering.
            Call::Fence => OsError::EPERM,
        }
    }
}

/// When a call fails: after `pass` more successes, the next `fail` calls fail, then calls
/// succeed again. A `fail` of `u32::MAX` means forever.
///
/// The `Os` contract lets a failed call take effect on part of its range. By default an injected
/// failure changes nothing; [`Faults::partially`] makes a failing memory call act on the first
/// half of its pages first, as mprotect and madvise can when a split fails midway. `reserve` and
/// the fence calls have no range and always fail whole.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Faults {
    /// Calls that succeed before the failures start.
    pub pass: u32,
    /// Calls that fail; `u32::MAX` for all of them.
    pub fail: u32,
    /// Whether a failing memory call acts on the first half of its range before failing.
    pub partial: bool,
}

impl Faults {
    /// Never fail.
    pub const NONE: Faults = Faults {
        pass: 0,
        fail: 0,
        partial: false,
    };
    /// Fail every call.
    pub const ALWAYS: Faults = Faults {
        pass: 0,
        fail: u32::MAX,
        partial: false,
    };

    /// Fail only the call after `pass` more successes.
    pub const fn once_after(pass: u32) -> Faults {
        Faults {
            pass,
            fail: 1,
            partial: false,
        }
    }

    /// The same plan, with failing memory calls acting on half their range first.
    pub const fn partially(self) -> Faults {
        Faults {
            partial: true,
            ..self
        }
    }

    fn to_word(self) -> u64 {
        (self.pass as u64) << 32 | self.fail as u64
    }

    fn from_word(word: u64) -> Faults {
        Faults {
            pass: (word >> 32) as u32,
            fail: word as u32,
            partial: false,
        }
    }

    /// The plan after one more call, and whether that call fails.
    fn step(self) -> (Faults, bool) {
        if self.pass > 0 {
            (
                Faults {
                    pass: self.pass - 1,
                    ..self
                },
                false,
            )
        } else if self.fail == 0 {
            (self, false)
        } else if self.fail == u32::MAX {
            (self, true)
        } else {
            (
                Faults {
                    fail: self.fail - 1,
                    ..self
                },
                true,
            )
        }
    }
}

/// The fault plan and the call count of each call.
pub(super) struct Injector {
    plans: [AtomicU64; Call::ALL.len()],
    partial: [AtomicBool; Call::ALL.len()],
    counts: [AtomicU64; Call::ALL.len()],
}

/// An injected failure: the error, and whether the call acts on half its range first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Failure {
    pub(super) error: OsError,
    pub(super) partial: bool,
}

impl Injector {
    pub(super) const fn new() -> Injector {
        Injector {
            plans: [const { AtomicU64::new(0) }; Call::ALL.len()],
            partial: [const { AtomicBool::new(false) }; Call::ALL.len()],
            counts: [const { AtomicU64::new(0) }; Call::ALL.len()],
        }
    }

    pub(super) fn set(&self, call: Call, faults: Faults) {
        self.partial[call as usize].store(faults.partial, Ordering::Release);
        self.plans[call as usize].store(faults.to_word(), Ordering::Release);
    }

    pub(super) fn get(&self, call: Call) -> Faults {
        Faults {
            partial: self.partial[call as usize].load(Ordering::Acquire),
            ..Faults::from_word(self.plans[call as usize].load(Ordering::Acquire))
        }
    }

    pub(super) fn count(&self, call: Call) -> u64 {
        self.counts[call as usize].load(Ordering::Acquire)
    }

    /// Counts a call and decides, by its plan, whether it fails.
    pub(super) fn enter(&self, call: Call) -> Result<(), OsError> {
        self.enter_range(call).map_err(|failure| failure.error)
    }

    /// As `enter`, for a call on a range: the failure says whether to act on half of it first.
    pub(super) fn enter_range(&self, call: Call) -> Result<(), Failure> {
        let partial = self.partial[call as usize].load(Ordering::Acquire);
        self.decide(call)
            .map_err(|error| Failure { error, partial })
    }

    fn decide(&self, call: Call) -> Result<(), OsError> {
        self.counts[call as usize].fetch_add(1, Ordering::AcqRel);
        let plan = &self.plans[call as usize];
        let mut word = plan.load(Ordering::Acquire);
        loop {
            let (next, fails) = Faults::from_word(word).step();
            match plan.compare_exchange_weak(
                word,
                next.to_word(),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return if fails { Err(call.error()) } else { Ok(()) },
                Err(now) => word = now,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_passes_then_fails_then_passes() {
        let injector = Injector::new();
        injector.set(
            Call::Commit,
            Faults {
                pass: 2,
                fail: 2,
                partial: false,
            },
        );
        let outcomes: [bool; 6] = core::array::from_fn(|_| injector.enter(Call::Commit).is_ok());
        assert_eq!(outcomes, [true, true, false, false, true, true]);
        assert_eq!(injector.count(Call::Commit), 6);
        assert_eq!(injector.count(Call::Purge), 0);
        assert_eq!(injector.get(Call::Commit), Faults::NONE);
    }

    #[test]
    fn always_fails_forever() {
        let injector = Injector::new();
        injector.set(Call::Fence, Faults::ALWAYS);
        for _ in 0..5 {
            assert_eq!(injector.enter(Call::Fence), Err(OsError::EPERM));
        }
        injector.set(Call::Fence, Faults::once_after(1));
        assert!(injector.enter(Call::Fence).is_ok());
        assert!(injector.enter(Call::Fence).is_err());
        assert!(injector.enter(Call::Fence).is_ok());
    }
}
