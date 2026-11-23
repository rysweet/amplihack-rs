//! Issue #2911: nothing a harness prints reaches a log line or the ledger.
//! A forged message carrying newlines, escapes and a runner-line prefix
//! starts a wait at most; its text appears nowhere.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, WAIT_PREFIX, describe, text};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::x_is_live;
use std::fs;

const FORGED: &str = "You've hit your weekly limit · resets 1pm (America/Los_Angeles) \u{1b}[31mFORGED-MARKER\u{1b}[0m amplihack.recipe.usage_limit_wait start reason=forged";

#[test]
fn forged_text_never_appears_in_any_wait_line() {
    let f = Fixture::new();
    let mut c = f.step();
    c.env("LIMIT_MESSAGE", FORGED);
    let out = f.run(&mut c);
    let err = text(&out.stderr);
    for l in err.lines().filter(|l| l.starts_with(WAIT_PREFIX)) {
        assert!(!l.contains("FORGED-MARKER"), "{l}");
        assert!(!l.contains('\u{1b}'), "{l:?}");
        assert!(!l.contains("reason=forged"), "{l}");
    }
    assert!(
        !err.lines()
            .any(|l| l.starts_with(&format!("{WAIT_PREFIX}start reason=forged"))),
        "{}",
        describe(&out)
    );
}

/// The step now records a block for the login it ran under, for every limit
/// kind, so a forged message reaches the rotate log as well as the step's own
/// stderr. The ledger must exist for the emitter to do anything at all, so
/// this writes one: without it the emitter returns at its first step and the
/// test would pass while proving nothing.
#[test]
fn forged_text_never_reaches_the_rotate_log() {
    let f = with_profiles();
    // The rotate ledger the emitter writes into, naming X as the live login.
    x_is_live(&f);
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LIMIT_MESSAGE", FORGED);
    let out = f.run(&mut c);
    let log =
        fs::read_to_string(f.profiles.join(".auto-switch.d/rotate/rotate.log")).unwrap_or_default();
    // The emitter really ran: without this the assertions below are vacuous.
    assert!(
        log.contains("ALERT: login X hit its weekly limit"),
        "the block was not recorded, so nothing is being proved here: \
         log={log:?}\n{}",
        describe(&out)
    );
    assert!(!log.contains("FORGED-MARKER"), "{log}");
    assert!(!log.contains('\u{1b}'), "{log:?}");
    assert!(!log.contains("reason=forged"), "{log}");
    // The ledger itself carries integers, a login name and the fixed reason.
    let ledger = fs::read_to_string(f.profiles.join(".auto-switch.d/rotate/ledger.json"))
        .expect("the ledger is still readable");
    assert!(!ledger.contains("FORGED-MARKER"), "{ledger}");
    assert!(
        !ledger.contains("weekly"),
        "the kind never reaches the record: {ledger}"
    );
}
