//! Issue #1890: `amplihack profile rotate --recovery act|log|off`, end to end.
//! Every command runs through `isolated(..)` (by way of
//! `support::profile_rotate::Fixture`) with a temporary HOME, a temporary
//! profile root, fake profiles, fake login files and a stub `claude`; the
//! child gets no `AMPLIHACK_ROTATE_RECOVERY` and no real HOME or profile
//! root from this process. Logins are checked by inode only.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;
use std::path::Path;

const RECOVERY_ENV: &str = "AMPLIHACK_ROTATE_RECOVERY";

/// Nothing ran: every login where it was, no rotate ledger, no backup.
fn nothing_touched(f: &Fixture) {
    assert!(f.home.starts_with(f.tmp.path()), "temporary HOME only");
    assert!(f.root.starts_with(f.tmp.path().canonicalize().unwrap()));
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    assert_eq!(ino(&f.slot("spare")), Some(f.spare_ino));
    assert_eq!(ino(&f.slot("extra")), Some(f.extra_ino));
    assert!(f.ledger().is_none(), "no rotate ledger written");
    assert!(!Path::new(&f.root).join(".rejected-logins").exists());
}

#[test]
fn an_invalid_recovery_flag_fails_before_any_cycle_work() {
    let f = Fixture::new();
    let out = f
        .cmd(&["profile", "rotate", "--recovery", "bogus"])
        .env_remove(RECOVERY_ENV)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let err = text(&out.stderr);
    assert!(
        err.contains("invalid rotate setting --recovery \"bogus\": expected act, log or off"),
        "{err}"
    );
    assert!(
        err.contains("hint: run `amplihack profile rotate --help`"),
        "{err}"
    );
    nothing_touched(&f);
}

#[test]
fn an_invalid_recovery_variable_is_named_and_upper_case_is_rejected() {
    let f = Fixture::new();
    let out = f
        .cmd(&["profile", "rotate"])
        .env(RECOVERY_ENV, "LOG")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(
        err.contains("invalid rotate setting AMPLIHACK_ROTATE_RECOVERY \"LOG\""),
        "{err}"
    );
    nothing_touched(&f);
}

#[test]
fn each_valid_recovery_value_is_accepted_by_dry_run_and_writes_nothing() {
    let f = Fixture::new();
    let plain = f.run(&["profile", "rotate", "--dry-run"]);
    for word in ["act", "log", "off"] {
        let out = f.run(&["profile", "rotate", "--dry-run", "--recovery", word]);
        assert_eq!(
            out.status.code(),
            plain.status.code(),
            "--recovery {word}: {}",
            text(&out.stderr)
        );
        assert!(!text(&out.stderr).contains("invalid rotate setting"));
        nothing_touched(&f);
    }
}
