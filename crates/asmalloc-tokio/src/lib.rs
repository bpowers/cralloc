//! asmalloc's maintenance from a task on a tokio runtime.
//!
//! asmalloc does its maintenance (returning memory to the OS, emptying idle caches) in the
//! slow paths of allocation, so a server that falls silent keeps what its last burst left
//! until the next burst begins, and that burst's first requests then pay for the maintenance
//! the silence skipped. A task that wakes once an interval and calls
//! [`asmalloc::Asmalloc::maintain`] does that work during the silence instead, for one wake-up
//! a second. While the server is busy the call returns at once, since the allocator's own
//! heartbeats have run in the interval.
//!
//! ```no_run
//! #[global_allocator]
//! static GLOBAL: asmalloc::Asmalloc = asmalloc::Asmalloc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let _maintenance = asmalloc_tokio::spawn_maintenance();
//!     // ... serve ...
//! }
//! ```
//!
//! Use this in a program that runs a tokio runtime for its whole life. A program without one,
//! or one whose runtime may stop while the program goes on, uses asmalloc's opt-in background
//! thread instead (`Asmalloc::start_background`, or `ASMALLOC_BACKGROUND=1` for the preload
//! library). Running both is harmless: `maintain` runs at most one round an interval, whoever
//! calls it. Neither survives a `fork`: the child has neither the task's runtime threads nor
//! the background thread.

use asmalloc::Asmalloc;
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval};

/// Spawns a task on the current tokio runtime that calls [`Asmalloc::maintain`] every
/// [`Asmalloc::MAINTAIN_EVERY`], a fraction of an interval so that every interval gets a call,
/// until the runtime shuts down or the returned handle is aborted.
///
/// # Panics
///
/// If called outside a tokio runtime, as `tokio::spawn` does.
pub fn spawn_maintenance() -> JoinHandle<()> {
    tokio::spawn(async {
        let mut every = interval(Asmalloc::MAINTAIN_EVERY);
        // A runtime too busy to wake the task on time needs no catching up: the heartbeats
        // of a busy allocator do the work.
        every.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            Asmalloc::maintain();
        }
    })
}

#[cfg(test)]
mod tests {
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn the_task_runs_until_aborted() {
        let handle = super::spawn_maintenance();
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert!(!handle.is_finished());
        handle.abort();
        assert!(handle.await.unwrap_err().is_cancelled());
    }
}
