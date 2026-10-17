//! A fork child's failures reach the parent: a model failure with its kind, a crash, and a
//! hang. The subjects misbehave only in the child, and without locks, so the parent's run is
//! clean and the child cannot deadlock on state another thread held at the fork.
#![cfg(all(target_os = "linux", not(miri)))]

use std::time::Duration;

use asmalloc_testkit::subject::{Grant, Held, Refused, Request, Resized, Subject, ZeroSize};
use asmalloc_testkit::{Config, FailureKind, ForkPlan, Handoff, Profile, run, target};

#[derive(Clone, Copy)]
enum Misbehaviour {
    /// Zeroed requests are served unzeroed.
    SkipZeroing,
    /// The first allocation never returns.
    Hang,
    /// The first allocation kills the process.
    Crash,
}

struct InChild<S> {
    inner: S,
    parent: u32,
    how: Misbehaviour,
}

unsafe extern "C" {
    fn raise(sig: i32) -> i32;
}

impl<S: Subject> InChild<S> {
    fn new(inner: S, how: Misbehaviour) -> Self {
        InChild {
            inner,
            parent: std::process::id(),
            how,
        }
    }

    fn in_child(&self) -> bool {
        std::process::id() != self.parent
    }
}

impl<S: Subject> Subject for InChild<S> {
    fn name(&self) -> String {
        format!("child-only fault in {}", self.inner.name())
    }

    fn zero_size(&self) -> ZeroSize {
        self.inner.zero_size()
    }

    unsafe fn alloc(&self, req: Request) -> Result<Option<Grant>, Refused> {
        if !self.in_child() {
            // SAFETY: same contract.
            return unsafe { self.inner.alloc(req) };
        }
        match self.how {
            Misbehaviour::SkipZeroing => {
                let plain = Request {
                    zeroed: false,
                    ..req
                };
                // SAFETY: same contract.
                let g = unsafe { self.inner.alloc(plain) }?;
                Ok(g.map(|g| Grant {
                    zeroed: if req.zeroed { req.size } else { 0 },
                    ..g
                }))
            }
            Misbehaviour::Hang => loop {
                std::thread::sleep(Duration::from_millis(10));
            },
            Misbehaviour::Crash => {
                // SAFETY: raising SIGKILL has no preconditions; it ends the child.
                unsafe { raise(9) };
                unreachable!("SIGKILL cannot be caught")
            }
        }
    }

    unsafe fn free(&self, block: Held, variant: u32) {
        // SAFETY: same contract.
        unsafe { self.inner.free(block, variant) }
    }

    unsafe fn resize(&self, block: Held, req: Request) -> Result<Resized, Refused> {
        // SAFETY: same contract.
        unsafe { self.inner.resize(block, req) }
    }

    unsafe fn usable_size(&self, block: Held) -> Option<usize> {
        // SAFETY: same contract.
        unsafe { self.inner.usable_size(block) }
    }
}

fn forking(timeout: Duration) -> Config {
    let plan = ForkPlan {
        at: 300,
        child_workers: 2,
        child_ops: 3000,
        timeout,
    };
    Config::threads(Profile::SMALL_FIXED, 3, Handoff::AllToAll, 21, 1000).with_fork(plan)
}

#[test]
fn a_model_failure_in_the_child_reaches_the_parent_with_its_kind() {
    let s = InChild::new(target::global_alloc(), Misbehaviour::SkipZeroing);
    let f = run(&s, &forking(Duration::from_secs(60))).expect_err("the child must fail");
    assert_eq!(f.kind, FailureKind::NotZeroed, "{f}");
    assert!(f.detail.contains("in the child"), "{f}");
}

#[test]
fn a_hung_child_is_killed_and_reported() {
    let s = InChild::new(target::c_abi(), Misbehaviour::Hang);
    let f = run(&s, &forking(Duration::from_millis(300))).expect_err("the child must hang");
    assert_eq!(f.kind, FailureKind::ChildHung, "{f}");
}

#[test]
fn a_crashed_child_is_reported() {
    let s = InChild::new(target::global_alloc(), Misbehaviour::Crash);
    let f = run(&s, &forking(Duration::from_secs(60))).expect_err("the child must crash");
    assert_eq!(f.kind, FailureKind::ChildCrashed, "{f}");
    assert!(f.detail.contains("signal 9"), "{f}");
}
