//! Issue #1606: a signal to `recipe run` stops the whole recipe tree.
//!
//! The runner is started in its own session (`setsid`, so a timeout can
//! signal its whole process group). That also puts it out of reach of every
//! signal aimed at `recipe run`: `tmux kill-session` (SIGHUP), closing the
//! terminal (SIGHUP), Ctrl-C (SIGINT, delivered to the terminal's foreground
//! group only) and `kill` (SIGTERM). Before this module `recipe run` died of
//! the signal and left the runner and its agent children running under
//! PID 1. Now the signal is recorded, and the wait loop tears the runner's
//! group down the way a timeout does, then exits with `128 + signal`.

use std::fmt;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The signals that stop a run.
#[cfg(unix)]
const STOP_SIGNALS: [libc::c_int; 3] = [libc::SIGHUP, libc::SIGINT, libc::SIGTERM];

/// The last stop signal received, or 0. Registered once per process: the
/// handlers stay for the life of `recipe run`, which ends with the run.
fn received() -> &'static Arc<AtomicUsize> {
    static RECEIVED: OnceLock<Arc<AtomicUsize>> = OnceLock::new();
    RECEIVED.get_or_init(|| {
        let flag = Arc::new(AtomicUsize::new(0));
        #[cfg(unix)]
        for signal in STOP_SIGNALS {
            if let Err(error) =
                signal_hook::flag::register_usize(signal, Arc::clone(&flag), signal as usize)
            {
                tracing::warn!(signal, %error, "could not trap a stop signal for recipe run");
            }
        }
        flag
    })
}

/// Start trapping the stop signals. Called once at the start of a run, so a
/// signal at any point after it (setup, a running attempt, retry backoff) is
/// seen by the next check.
pub(crate) fn arm() {
    let _ = received();
}

/// The stop signal received since [`arm`], if any.
pub(crate) fn pending() -> Option<i32> {
    pending_in(received())
}

fn pending_in(flag: &AtomicUsize) -> Option<i32> {
    match flag.load(Ordering::SeqCst) {
        0 => None,
        signal => i32::try_from(signal).ok(),
    }
}

/// Sleep for `delay`, but return as soon as a stop signal is pending. Used
/// for retry backoff, which can last minutes.
pub(super) fn sleep_unless_stopped(delay: std::time::Duration) {
    sleep_unless(received(), delay);
}

fn sleep_unless(flag: &AtomicUsize, delay: std::time::Duration) {
    const SLICE: std::time::Duration = std::time::Duration::from_millis(100);
    let deadline = std::time::Instant::now() + delay;
    while pending_in(flag).is_none() {
        let now = std::time::Instant::now();
        if now >= deadline {
            return;
        }
        std::thread::sleep(SLICE.min(deadline - now));
    }
}

/// `recipe run` was stopped by `signal` and has torn the runner down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoppedBySignal(pub(crate) i32);

impl StoppedBySignal {
    /// The conventional exit status for death by `signal`.
    pub(crate) fn exit_code(self) -> i32 {
        128 + self.0
    }
}

impl fmt::Display for StoppedBySignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "recipe run stopped by signal {}; the recipe runner and its process group were \
             terminated",
            self.0
        )
    }
}

impl std::error::Error for StoppedBySignal {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_sleep_ends_early_when_a_stop_is_pending() {
        let flag = AtomicUsize::new(0);
        let started = std::time::Instant::now();
        sleep_unless(&flag, std::time::Duration::from_millis(200));
        assert!(started.elapsed() >= std::time::Duration::from_millis(200));

        flag.store(2, Ordering::SeqCst);
        let started = std::time::Instant::now();
        sleep_unless(&flag, std::time::Duration::from_secs(60));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(pending_in(&flag), Some(2));
    }

    #[test]
    fn exit_code_is_128_plus_the_signal() {
        assert_eq!(StoppedBySignal(1).exit_code(), 129);
        assert_eq!(StoppedBySignal(15).exit_code(), 143);
    }
}
