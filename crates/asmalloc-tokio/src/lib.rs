//! asmalloc's maintenance from a task on a tokio runtime.
//!
//! asmalloc does its maintenance (returning memory to the OS, emptying idle caches) in the
//! slow paths of allocation, so a server that falls silent keeps what its last burst left
//! until the next burst begins. A task that wakes about twice a second and takes the round of
//! [`asmalloc::Asmalloc::maintain`] does that work during the silence instead. It takes the
//! round a bounded step at a time ([`asmalloc::Asmalloc::maintain_step`], tens of
//! microseconds at most) and yields to the runtime between steps, so it never holds a worker
//! for longer than a step; a call in an interval whose round is done returns at once.
//!
//! ```no_run
//! #[global_allocator]
//! static GLOBAL: asmalloc::Asmalloc = asmalloc::Asmalloc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let _maintenance = asmalloc_tokio::spawn_maintenance().expect("a runtime with a timer");
//!     // ... serve ...
//! }
//! ```
//!
//! Use this in a program that runs a tokio runtime with its timer enabled (`enable_time`, which
//! `#[tokio::main]` does) for its whole life. A program without one, or one whose runtime may
//! stop while the program goes on, leaves the work to asmalloc's background thread instead,
//! which starts by default in a program with threads of its own
//! (`Asmalloc::start_background` starts it at once).
//! Running both is harmless: a round runs once an interval, whoever takes its steps. Neither
//! survives a `fork`: the child has neither the task's runtime threads nor the background
//! thread.

use asmalloc::Asmalloc;
use std::fmt;
use tokio::task::JoinHandle;
use tokio::time::{Interval, MissedTickBehavior, interval};

/// The runtime the call was made on has no timer (`enable_time` was not called on its
/// builder), so the task could never wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoTimer;

impl fmt::Display for NoTimer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("asmalloc-tokio needs a tokio runtime with its timer enabled (enable_time)")
    }
}

impl std::error::Error for NoTimer {}

/// Spawns a task on the current tokio runtime that takes a round of asmalloc's maintenance
/// every [`Asmalloc::MAINTAIN_EVERY`], a fraction of an interval so that every interval gets
/// one, a bounded step at a time, yielding between steps; until the runtime shuts down or the
/// returned handle is aborted.
///
/// # Errors
///
/// [`NoTimer`] if the runtime has no timer. Finding that out needs tokio's panic in the
/// caller, which the call catches; a program built with `panic = "abort"` aborts there
/// instead, at the call and not later in the task.
///
/// # Panics
///
/// If called outside a tokio runtime, as `tokio::spawn` does.
pub fn spawn_maintenance() -> Result<JoinHandle<()>, NoTimer> {
    spawn_with(Asmalloc::maintain_step)
}

/// [`spawn_maintenance`] with the step to take, so that a test can count the steps.
fn spawn_with(step: fn() -> bool) -> Result<JoinHandle<()>, NoTimer> {
    let every =
        std::panic::catch_unwind(|| interval(Asmalloc::MAINTAIN_EVERY)).map_err(|_| NoTimer)?;
    Ok(tokio::spawn(run(every, step)))
}

async fn run(mut every: Interval, step: fn() -> bool) {
    // A runtime too busy to wake the task on time needs no catching up: a round runs once an
    // interval whoever calls it.
    every.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        while step() {
            tokio::task::yield_now().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    /// A round of three steps: two that leave more, one that ends it.
    static STEPS: AtomicUsize = AtomicUsize::new(0);
    fn three_step_round() -> bool {
        !STEPS
            .fetch_add(1, Relaxed)
            .wrapping_add(1)
            .is_multiple_of(3)
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn the_task_takes_a_round_of_steps_each_tick_until_aborted() {
        let handle = super::spawn_with(three_step_round).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1250)).await;
        // Ticks at 0, 500 and 1000 ms: three rounds of three steps.
        assert_eq!(STEPS.load(Relaxed), 9);
        assert!(!handle.is_finished());
        handle.abort();
        assert!(handle.await.unwrap_err().is_cancelled());
    }

    #[test]
    fn the_real_step_runs_on_a_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();
        runtime.block_on(async {
            let handle = super::spawn_maintenance().unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            handle.abort();
        });
    }

    /// Review of `burst-start`: on a runtime built without its timer (`enable_time`), the call
    /// must refuse, not hand back a task that dies at its first poll.
    #[test]
    fn a_runtime_without_a_timer_is_refused_at_the_call_not_silently_in_the_task() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let outcome = runtime.block_on(async { super::spawn_maintenance() });
        assert_eq!(outcome.err(), Some(super::NoTimer));
    }
}
