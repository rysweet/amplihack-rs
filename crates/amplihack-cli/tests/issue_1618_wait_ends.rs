//! Issue #1618: how the usage-limit wait ends other than at the reset.
//!
//! - AC4: `max_wait_secs = 0` waits zero seconds and still retries once;
//!   a step timeout of 60 s or less ends the wait at once with no retry.
//! - AC5: a login that changes during the wait ends it with
//!   `login changed`, then the retry runs.
//!
//! See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, WAIT_PREFIX, claude_stdout, describe, events};
use std::fs;

#[test]
fn ac4_a_zero_maximum_waits_zero_seconds_and_retries_once() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "0");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:00:00 -07:00 bound=max_wait",
            "end reason=max reached",
            "resume attempt=2",
        ]
    );
    assert_eq!(f.count("0"), 2);
}

#[test]
fn ac4_a_smaller_maximum_names_the_bound() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "600");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out)[0],
        "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
         until=2026-10-18 08:10:00 -07:00 bound=max_wait"
    );
}

#[test]
fn ac4_a_larger_maximum_changes_nothing() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "86400");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        events(&out)[0].ends_with("until=2026-10-18 08:32:00 -07:00 bound=reset+margin"),
        "{:?}",
        events(&out)
    );
}

#[test]
fn ac4_a_step_timeout_of_sixty_seconds_or_less_ends_the_step_without_a_retry() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_STEP_TIMEOUT", "30");
    let out = f.run(&mut c);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(f.count("0"), 1, "no retry after a step timeout");
    let ev = events(&out);
    assert_eq!(ev.len(), 3, "{ev:?}");
    assert!(
        ev[0].starts_with(
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 until="
        ),
        "{ev:?}"
    );
    assert!(ev[0].ends_with(" bound=step_timeout"), "{ev:?}");
    assert_eq!(ev[1], "end reason=step timeout");
    // The closing line keeps the step classed usage_limit (#1801).
    assert_eq!(ev[2], "failed reason=step_timeout: usage limit not lifted");
    assert_eq!(
        claude_stdout(&out),
        "attempt 1 output\n",
        "the failed attempt's held-back stdout is printed"
    );
}

#[test]
fn a_zero_step_timeout_does_not_bound_the_wait() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_STEP_TIMEOUT", "0");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        events(&out)[0].ends_with("bound=reset+margin"),
        "{:?}",
        events(&out)
    );
}

#[test]
fn ac5_a_login_moved_into_place_during_the_wait_ends_it() {
    let f = Fixture::new();
    let mut c = f.step();
    // Issue #1973: the test moves the login into place itself, once the
    // wait's first `progress` line shows two polls have agreed on the old
    // state, rather than a fixed second after the first attempt. 3x: a 30 s
    // virtual poll is 10 s real, and the 1920 s wait 640 s real, so on a
    // loaded host the wait cannot reach the reset before the change. The
    // `end reason=login changed` event proves it ended at the change, not
    // the reset (an elapsed-time check was removed). If the change never
    // ends the wait, `run_watching` kills the step at the shared 10-minute
    // ceiling; the scale does not bound a failing run.
    c.env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "3");
    let creds = f.config.join(".credentials.json");
    let progress = format!("{WAIT_PREFIX}progress ");
    let mut moved = false;
    let out = f.run_watching(&mut c, |line| {
        if !moved && line.starts_with(&progress) {
            fs::write(&creds, "").expect("move a login into place");
            moved = true;
        }
    });
    assert!(moved, "the wait wrote no progress line: {}", describe(&out));
    assert!(out.status.success(), "{}", describe(&out));
    let ev = events(&out);
    assert_eq!(
        ev[1..],
        ["end reason=login changed", "resume attempt=2"],
        "{ev:?}"
    );
    assert_eq!(f.count("0"), 2);
}

/// Issue #1743: a finished wait appends its real length to the ledger that
/// auto-drive names, so its wall budget can leave the wait out.
#[test]
fn a_finished_wait_is_written_to_the_ledger_auto_drive_names() {
    let f = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let ledger = dir.path().join("ledger");
    let mut c = f.step();
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "0")
        .env("AMPLIHACK_USAGE_WAIT_LEDGER", &ledger);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(events(&out)[1], "end reason=max reached");
    let text = std::fs::read_to_string(&ledger).expect("ledger written");
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert!(text.trim_end().parse::<u64>().is_ok(), "{text:?}");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&ledger).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
