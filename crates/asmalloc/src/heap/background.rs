//! The background thread, for a Rust program (feature `background`): an ordinary detached
//! thread of the process that sleeps [`Asmalloc::MAINTAIN_EVERY`] and calls
//! [`Asmalloc::maintain`], forever.
//!
//! The feature brings in `std` for its threads; without it the crate stays `no_std` and starts
//! no thread. The thread holds no lock and no allocator state: at any instant it is asleep or
//! in `maintain`, whose every step is one a slow path takes, so process exit and a fork find
//! it in a state they already handle. The child of a fork has no such thread, and may start
//! its own. The preload library starts the same loop with the C library's threads
//! (`crates/asmalloc-preload`).

use super::global::Asmalloc;
use core::sync::atomic::{AtomicU32, Ordering};
use std::io;
use std::thread;

/// The ID of the process that started the thread; zero if none has. A forked child inherits
/// its parent's, which is not its own, so it can start one of its own. A child that has its
/// parent's ID all the same (PID 1 of a new PID namespace forked by PID 1 of another, or a
/// reused ID) is taken for the parent, and its call starts nothing.
static STARTED: AtomicU32 = AtomicU32::new(0);

impl Asmalloc {
    /// Starts the background thread that calls [`Asmalloc::maintain`] about twice a second,
    /// so that the heap returns idle memory even when no thread takes a slow path. `maintain`
    /// runs one round an interval and returns at once otherwise; its clock read is what counts
    /// the intervals of a silence (`crate::clock`). The thread
    /// is named `asmalloc`, is detached, and runs until the process exits.
    ///
    /// Call it once, early in `main`. A second call, from any thread, returns `Ok` and starts
    /// nothing; so does one made while another thread's call is starting the thread. The child
    /// of a fork does not inherit the thread, and a call in the child starts one there. The
    /// error is the one `std::thread::Builder::spawn` gave, and leaves no thread started.
    pub fn start_background() -> io::Result<()> {
        let me = std::process::id();
        let seen = STARTED.load(Ordering::Acquire);
        if seen == me
            || STARTED
                .compare_exchange(seen, me, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Ok(());
        }
        match thread::Builder::new()
            .name("asmalloc".into())
            .spawn(maintain_forever)
        {
            // Dropping the handle detaches the thread.
            Ok(_) => Ok(()),
            Err(error) => {
                let _ = STARTED.compare_exchange(me, seen, Ordering::AcqRel, Ordering::Relaxed);
                Err(error)
            }
        }
    }
}

fn maintain_forever() {
    loop {
        thread::sleep(Asmalloc::MAINTAIN_EVERY);
        Asmalloc::maintain();
    }
}
