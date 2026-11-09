//! Issue #2018: the headless-step predicate and the D1 exit code.
//!
//! Included from `step_lifetime.rs` as
//! `#[cfg(test)] #[path = "step_lifetime_tests.rs"] mod tests;`.
//!
//! The contract these tests fix for `step_lifetime`:
//!
//! - `is_headless_step(run_id: Option<&str>, stdin_tty: bool,
//!   stdout_tty: bool) -> bool`: true when `run_id` is non-empty and stdin
//!   and stdout are not both terminals. Nothing else is an input.
//! - `headless_signal_exit_code(first: Option<i32>) -> i32`: `128 + N` for
//!   SIGTERM, SIGINT and SIGHUP, and 143 for anything else or `None`.
//!
//! Both are pure, so these tests never change the process environment and
//! never call `prctl` on the test process. `bind_launcher_if_headless` is
//! covered end to end by `tests/issue_2018_step_lifetime.rs`.

use super::{headless_signal_exit_code, is_headless_step};

const RUN_ID: Option<&str> = Some("5b60657b-76ef-4f49-8a22-8b89ed75f43e");

const ALL_TERMINAL_COMBINATIONS: [(bool, bool); 4] =
    [(false, false), (true, false), (false, true), (true, true)];

// ---------------------------------------------------------------------
// The predicate
// ---------------------------------------------------------------------

#[test]
fn no_run_id_is_never_headless() {
    for (stdin_tty, stdout_tty) in ALL_TERMINAL_COMBINATIONS {
        assert!(
            !is_headless_step(None, stdin_tty, stdout_tty),
            "no run id, stdin tty {stdin_tty}, stdout tty {stdout_tty}"
        );
    }
}

#[test]
fn an_empty_run_id_is_never_headless() {
    for (stdin_tty, stdout_tty) in ALL_TERMINAL_COMBINATIONS {
        assert!(
            !is_headless_step(Some(""), stdin_tty, stdout_tty),
            "empty run id, stdin tty {stdin_tty}, stdout tty {stdout_tty}"
        );
    }
}

#[test]
fn a_terminal_on_both_stdin_and_stdout_is_interactive() {
    assert!(!is_headless_step(RUN_ID, true, true));
}

/// The usual case when `recipe run` is started from a terminal: the runner
/// passes stdin through and sends the step's stdout to a file.
#[test]
fn stdin_on_a_terminal_with_stdout_to_a_file_is_headless() {
    assert!(is_headless_step(RUN_ID, true, false));
}

#[test]
fn null_stdin_with_stdout_to_a_file_is_headless() {
    assert!(is_headless_step(RUN_ID, false, false));
}

#[test]
fn null_stdin_with_stdout_on_a_terminal_is_headless() {
    assert!(is_headless_step(RUN_ID, false, true));
}

/// The validity of the run id is a record concern (D6), not a lifetime one:
/// a run id the record rejects still makes the launch a headless step, so
/// the binding still applies.
#[test]
fn a_run_id_the_record_rejects_is_still_headless() {
    for id in ["../not a valid id", ".", "x".repeat(129).as_str()] {
        assert!(is_headless_step(Some(id), false, false), "{id:?}");
    }
}

/// `AMPLIHACK_NONINTERACTIVE` plays no part: the predicate takes no such
/// input. This pins the signature so a later change has to come through
/// here.
#[test]
fn the_predicate_takes_only_the_run_id_and_two_terminals() {
    let f: fn(Option<&str>, bool, bool) -> bool = is_headless_step;
    assert!(f(RUN_ID, false, false));
}

// ---------------------------------------------------------------------
// D1: the exit code of a headless launch stopped by a signal
// ---------------------------------------------------------------------

#[test]
fn sigterm_exits_143() {
    assert_eq!(headless_signal_exit_code(Some(libc::SIGTERM)), 143);
}

#[test]
fn sigint_exits_130() {
    assert_eq!(headless_signal_exit_code(Some(libc::SIGINT)), 130);
}

#[test]
fn sighup_exits_129() {
    assert_eq!(headless_signal_exit_code(Some(libc::SIGHUP)), 129);
}

/// The shutdown flag was set but the signal was not identified.
#[test]
fn an_unidentified_signal_exits_143() {
    assert_eq!(headless_signal_exit_code(None), 143);
}

/// Only the three registered signals are mapped; anything else, which the
/// handlers never record, falls back to 143 rather than inventing a code.
#[test]
fn a_signal_outside_the_three_exits_143() {
    for sig in [libc::SIGUSR1, libc::SIGKILL, 0, -1] {
        assert_eq!(headless_signal_exit_code(Some(sig)), 143, "signal {sig}");
    }
}

/// Every headless signal exit is a failure the runner can see: never 0, and
/// never a code a child could be mistaken for having returned normally
/// (1 to 127).
#[test]
fn a_headless_signal_exit_is_never_success() {
    for sig in [
        None,
        Some(libc::SIGTERM),
        Some(libc::SIGINT),
        Some(libc::SIGHUP),
    ] {
        let code = headless_signal_exit_code(sig);
        assert!(code > 128, "{sig:?} gave {code}");
    }
}
