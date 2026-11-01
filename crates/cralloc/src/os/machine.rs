//! The seam between a routine's Rust twin and the processor it runs on.
//!
//! A twin runs as its routine would, one step at a time: read the CPU number, load the slab
//! field, read or write the magazine, commit. Before each step it asks its [`Machine`] whether
//! the OS has interrupted the sequence (preempted, migrated or signalled the thread, or fenced
//! its CPU); if so it starts again from the top, as the real routine's abort handler does. The
//! machine also says which CPU number the thread reads in its rseq area.
//!
//! [`crate::os::sim::SimOs`] implements it for simulated threads that tests place on CPUs,
//! interrupt at a chosen step, and whose sequences [`crate::os::Os::fence`] aborts.

/// A step of a routine, before which the OS may interrupt it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// Reading the CPU number: the first step of every sequence.
    ReadCpu,
    /// Loading the slab field.
    Load,
    /// Reading or writing the magazine: the object slot for pop and push, the link for the
    /// stacks.
    Access,
    /// The committing store.
    Commit,
}

/// The OS interrupted the sequence before the step: it starts again from the top.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Aborted;

/// The processor a twin runs on.
pub trait Machine {
    /// The `cpu_id` field of the thread's rseq area: a CPU number, or negative without a
    /// registration.
    fn cpu_id(&self) -> i32;

    /// Called before each step of a sequence. `Err` means the OS stopped the thread (preempted,
    /// migrated or signalled it, or a fence hit its CPU) since the sequence began, so the
    /// sequence restarts. A machine that moves the thread to another CPU does so here, so the
    /// restart reads the new number.
    fn step(&self, step: Step) -> Result<(), Aborted>;

    /// Called right after the magazine access that follows a successful `Access` step. An
    /// aborted sequence executes nothing more, so a fence must not return between the step's
    /// check and the access: a machine whose threads run freely makes its fence wait from the
    /// step until this call. Every twin must call it after its access, and nothing else in
    /// between.
    fn accessed(&self) {}

    /// Called right after the committing store, which ends the sequence. A fence on the
    /// thread's CPU may be waiting for it: between a successful `Commit` step and this call the
    /// sequence is still in flight, as the real routine's last instruction is. Every twin must
    /// call it after its store, and nothing else in between.
    fn committed(&self) {}
}

impl<M: Machine + ?Sized> Machine for &M {
    fn cpu_id(&self) -> i32 {
        (**self).cpu_id()
    }

    fn step(&self, step: Step) -> Result<(), Aborted> {
        (**self).step(step)
    }

    fn accessed(&self) {
        (**self).accessed()
    }

    fn committed(&self) {
        (**self).committed()
    }
}

/// A thread that stays on one CPU and is never interrupted.
#[derive(Clone, Copy, Debug)]
pub struct Pinned(pub i32);

impl Machine for Pinned {
    fn cpu_id(&self) -> i32 {
        self.0
    }

    fn step(&self, _: Step) -> Result<(), Aborted> {
        Ok(())
    }
}
