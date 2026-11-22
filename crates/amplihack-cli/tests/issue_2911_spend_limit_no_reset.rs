//! Issue #2911: a spend limit that states no reset waits for a login with
//! room and retries, instead of `skipped reason=spend_limit`, with or
//! without a running rotator.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, SPEND_TEXT, describe, events};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::x_is_live;
use std::fs;
use std::process::Stdio;

#[test]
fn a_spend_limit_with_no_reset_waits_and_retries_with_no_rotator() {
    let f = with_profiles();
    x_is_live(&f);
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LIMIT_MESSAGE", SPEND_TEXT)
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "120");
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    let ev = events(&out);
    assert!(
        ev.iter()
            .any(|l| l.starts_with("waiting_for_capacity reason=spend_limit")),
        "{ev:?}"
    );
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped reason=spend_limit")),
        "{ev:?}"
    );
    assert!(!ev.iter().any(|l| l.starts_with("failed")), "{ev:?}");
}

/// `piped_prompt` stays a terminal skip, and a spend limit does not change
/// that (issue #2911 keeps both remaining skips).
///
/// A prompt genuinely piped on stdin is copied and replayed (#2815), so it
/// waits like any other. The unreplayable case is a `--input-format`
/// control channel: its caller holds it open for the length of the session,
/// so there is no end of it to copy and nothing a retry could be given.
/// Such a step ends with the limit result and `skipped reason=piped_prompt`
/// rather than announcing a retry that cannot run, however the limit that
/// met it reads.
#[test]
fn a_spend_limit_on_an_unreplayable_prompt_is_still_never_retried() {
    let f = Fixture::new();
    let mut c = f.cmd(&[
        "claude",
        "--subprocess-safe",
        "--",
        "-p",
        "--input-format",
        "stream-json",
    ]);
    c.env("LIMIT_MESSAGE", SPEND_TEXT)
        .env("LIMIT_CALLS", "1")
        // There is no prompt on the channel for the stub to read.
        .env("SKIP_STDIN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("start amplihack");
    // Held open by its caller, as a real control channel is.
    let held = child.stdin.take();
    let out = child.wait_with_output().expect("the step finished");
    drop(held);

    assert_eq!(f.count("0"), 1, "no rerun\n{}", describe(&out));
    assert_eq!(
        events(&out),
        vec!["skipped reason=piped_prompt".to_string()],
        "no wait was ever started\n{}",
        describe(&out)
    );
}
