//! Issue #1618: how the usage-limit wait ends other than at the reset.
//!
//! - AC4 (issue #2911): a wait whose limit stated no usable reset has no
//!   deadline, whatever the old length variable says, and ends on an event.
//!   No setting shortens a reset wait, which ends at its reset. A step
//!   timeout plays no part (issue #3037).
//! - AC5: a login that changes during the wait ends it with
//!   `login changed`, then the retry runs.
//!
//! See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, WAIT_PREFIX, describe, events};
use std::fs;

/// A session limit that states no reset: a capacity wait with no deadline.
const NO_RESET_TEXT: &str = "You've hit your session limit · try again later";

/// Issue #2911: a wait whose limit stated no reset has no deadline, and the
/// variable that used to bound it is not read, at any value. A login change
/// ends the wait and the step retries once.
#[test]
fn ac4_a_wait_with_no_reset_has_no_deadline_whatever_the_old_variable_says() {
    for value in ["0", "600", "86400"] {
        let f = Fixture::new();
        let mut c = f.step();
        c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", value)
            .env("LIMIT_MESSAGE", NO_RESET_TEXT);
        let out = f.run_with_a_login_change_per_open_wait(&mut c);
        assert!(out.status.success(), "{value}: {}", describe(&out));
        assert_eq!(
            events(&out),
            vec![
                "waiting_for_capacity reason=no_reset harness=claude until=none",
                "start reason=usage_limit wait=capacity harness=claude reset=none until=none \
                 bound=none",
                "end reason=login changed",
                "resume attempt=2",
            ],
            "{value}"
        );
        assert_eq!(f.count("0"), 2, "{value}");
    }
}

/// The old variable does not shorten a reset wait either: the wait ends at
/// its reset.
#[test]
fn the_old_variable_does_not_shorten_a_reset_wait() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "600");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        vec![
            "start reason=usage_limit harness=claude reset=2026-10-18 08:30:00 -07:00 \
             until=2026-10-18 08:32:00 -07:00 bound=reset+margin",
            "end reason=reset reached",
            "resume attempt=2",
        ]
    );
    assert_eq!(f.count("0"), 2);
}

/// Issue #3037: a step timeout does not end a limit
/// wait. This step used to stop a minute before a 30-second timeout, with
/// `failed reason=step_timeout` while its limit still stood, on the reading
/// that the recipe runner kills a step at that time. It does not:
/// recipe-runner-rs enforces only a step's own YAML `timeout:` field, and
/// the wait was the only code anywhere that turned the variable into an
/// ending. The wait now runs to its reset and the step retries.
#[test]
fn issue_3037_a_step_timeout_does_not_end_the_wait_or_stop_the_retry() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("AMPLIHACK_STEP_TIMEOUT", "30");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "the step retried after its wait");
    let ev = events(&out);
    assert!(
        ev[0].ends_with("until=2026-10-18 08:32:00 -07:00 bound=reset+margin"),
        "the deadline is the reset's, not the step timeout's: {ev:?}"
    );
    assert!(
        ev.iter()
            .all(|e| !e.contains("step timeout") && !e.contains("failed")),
        "no line says a timeout ended the wait or that the step failed: {ev:?}"
    );
    let err = crate::issue_1618_fixture::text(&out.stderr);
    assert!(
        !err.contains("AMPLIHACK_STEP_TIMEOUT"),
        "nothing on stderr names the step timeout as an ending\n{err}"
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
    c.env("AMPLIHACK_USAGE_WAIT_LEDGER", &ledger);
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(events(&out)[1], "end reason=reset reached");
    let text = std::fs::read_to_string(&ledger).expect("ledger written");
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert!(text.trim_end().parse::<u64>().is_ok(), "{text:?}");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&ledger).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
