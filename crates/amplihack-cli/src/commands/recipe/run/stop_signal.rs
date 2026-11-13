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

/// Issue #1976: the flag value for "the parent `recipe run` exited". No
/// signal number is this large, so the value alone says why the run stopped.
/// It is never passed to `kill`; teardown always sends SIGTERM then SIGKILL.
pub(crate) const ORPHANED: usize = 1 << 16;

/// Issue #1976: the exit status of an orphaned nested run (EX_TEMPFAIL).
pub(crate) const ORPHAN_EXIT_CODE: i32 = 75;

/// The parent an orphan stop was requested for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OrphanInfo {
    pub(crate) parent_pid: i32,
}

fn orphan_cell() -> &'static OnceLock<OrphanInfo> {
    static ORPHAN: OnceLock<OrphanInfo> = OnceLock::new();
    &ORPHAN
}

/// Stop the run because its parent `recipe run` exited. A stop signal that
/// arrived first is kept: the flag is only set when it is still clear.
#[cfg(target_os = "linux")]
pub(crate) fn request_orphan_stop(info: OrphanInfo) {
    let _ = orphan_cell().set(info);
    request_orphan_stop_in(received());
}

#[cfg(any(test, target_os = "linux"))]
pub(crate) fn request_orphan_stop_in(flag: &AtomicUsize) -> bool {
    flag.compare_exchange(0, ORPHANED, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// Whether the pending stop in `flag` is an orphan stop. A signal stored
/// after the orphan request replaces the value, and then this is false.
/// Production code decides from the value it captured with [`pending`].
#[cfg(test)]
pub(crate) fn is_orphaned_in(flag: &AtomicUsize) -> bool {
    pending_in(flag) == Some(ORPHANED as i32)
}

/// The parent recorded by [`request_orphan_stop`], if any.
pub(crate) fn orphan_info() -> Option<OrphanInfo> {
    orphan_cell().get().copied()
}

/// The one stderr line for an orphan stop (D10).
#[cfg(any(test, target_os = "linux"))]
pub(crate) fn orphan_stop_line(parent_pid: i32) -> String {
    format!("orphaned: parent recipe run {parent_pid} exited; stopping")
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
        if self.is_orphan() {
            ORPHAN_EXIT_CODE
        } else {
            128 + self.0
        }
    }

    /// Issue #1976: stopped because the parent `recipe run` exited.
    pub(crate) fn is_orphan(self) -> bool {
        self.0 == ORPHANED as i32
    }
}

impl fmt::Display for StoppedBySignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_orphan() {
            return match orphan_info() {
                Some(info) => write!(
                    f,
                    "recipe run stopped: parent recipe run {} exited; the recipe runner and \
                     its process group were terminated",
                    info.parent_pid
                ),
                None => write!(
                    f,
                    "recipe run stopped: its parent recipe run exited; the recipe runner and \
                     its process group were terminated"
                ),
            };
        }
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

    // Issue #1976: an orphaned nested run stops with its own reason. The flag
    // value is the only source of that reason (C-S6). These use the `_in`
    // forms on a private flag, as the tests above do, so they never touch the
    // process-wide flag.

    #[test]
    fn orphaned_is_a_value_no_signal_can_have() {
        assert_eq!(ORPHANED, 1 << 16);
        #[cfg(unix)]
        for signal in STOP_SIGNALS {
            assert_ne!(signal as usize, ORPHANED);
        }
    }

    #[test]
    fn an_orphan_stop_exits_75_and_a_signal_exits_128_plus_n() {
        assert_eq!(ORPHAN_EXIT_CODE, 75);
        assert_eq!(StoppedBySignal(ORPHANED as i32).exit_code(), 75);
        assert_eq!(StoppedBySignal(15).exit_code(), 143);
    }

    #[test]
    fn an_orphan_request_on_a_clear_flag_marks_the_run_orphaned() {
        let flag = AtomicUsize::new(0);
        assert!(!is_orphaned_in(&flag));
        assert!(request_orphan_stop_in(&flag));
        assert_eq!(pending_in(&flag), Some(ORPHANED as i32));
        assert!(is_orphaned_in(&flag));
    }

    #[test]
    fn an_orphan_request_keeps_a_signal_received_earlier() {
        let flag = AtomicUsize::new(0);
        flag.store(15, Ordering::SeqCst);
        assert!(!request_orphan_stop_in(&flag));
        assert_eq!(pending_in(&flag), Some(15));
        assert!(!is_orphaned_in(&flag));
        assert_eq!(StoppedBySignal(15).exit_code(), 143);
    }

    #[test]
    fn a_signal_after_an_orphan_request_wins_and_is_not_reported_orphaned() {
        // signal_hook stores unconditionally, so a later SIGTERM replaces
        // ORPHANED. The run must then report Failure and 143, never Orphaned.
        let flag = AtomicUsize::new(0);
        assert!(request_orphan_stop_in(&flag));
        flag.store(15, Ordering::SeqCst);
        assert!(!is_orphaned_in(&flag));
        let pending = pending_in(&flag).expect("a stop is pending");
        assert_eq!(StoppedBySignal(pending).exit_code(), 143);
    }

    #[test]
    fn a_second_orphan_request_changes_nothing() {
        let flag = AtomicUsize::new(0);
        assert!(request_orphan_stop_in(&flag));
        assert!(!request_orphan_stop_in(&flag));
        assert!(is_orphaned_in(&flag));
    }

    #[test]
    fn the_orphan_stop_message_names_the_parent() {
        assert_eq!(
            orphan_stop_line(412337),
            "orphaned: parent recipe run 412337 exited; stopping"
        );
    }
}
