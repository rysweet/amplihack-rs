//! Issue #2022: a usage limit whose reset is more than five hours away is
//! waited for until the reset plus the margin when nothing sets a maximum.
//! Before #2022 it was skipped (`reset_too_far`) and a capacity wait ended
//! at a six-hour default.
//!
//! The wait would last about a year of virtual time, so the run is ended by
//! SIGTERM once the `start` line is written. The step must report the
//! unclamped deadline, end with `end reason=signal`, and exit 143 with no
//! retry. See `issue_1618_fixture`.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, WAIT_PREFIX, WEEKLY_TEXT, describe, text};
use crate::issue_1618_wait_signals::signal_during_wait_with;
use crate::support::quiet_process::in_quiet_process;

const MAX_WAIT_ENV: &str = "AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS";

fn far_reset_with_no_maximum_waits_until_reset_plus_margin() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", WEEKLY_TEXT)
        .env_remove(MAX_WAIT_ENV)
        .env_remove("AMPLIHACK_STEP_TIMEOUT")
        .env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "1");
    let out = signal_during_wait_with(&f, c, libc::SIGTERM);

    assert_eq!(
        out.status.code(),
        Some(128 + libc::SIGTERM),
        "{}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 1, "no retry after a signal");
    let stderr = text(&out.stderr);
    let events: Vec<&str> = stderr
        .lines()
        .filter_map(|l| l.strip_prefix(WAIT_PREFIX))
        .filter(|l| !l.starts_with("progress"))
        .collect();
    assert_eq!(
        events,
        [
            "start reason=usage_limit harness=claude reset=2027-10-08 00:00:00 -07:00 \
             until=2027-10-08 00:02:00 -07:00 bound=reset+margin",
            "end reason=signal",
        ],
        "a far reset is a reset wait to reset + 120 s, not a skip"
    );
    assert!(
        !stderr.contains("reset_too_far"),
        "the reset_too_far skip is gone: {stderr}"
    );
}

#[test]
fn a_far_reset_with_no_maximum_waits_until_reset_plus_margin() {
    in_quiet_process(
        module_path!(),
        "a_far_reset_with_no_maximum_waits_until_reset_plus_margin",
        far_reset_with_no_maximum_waits_until_reset_plus_margin,
    );
}
