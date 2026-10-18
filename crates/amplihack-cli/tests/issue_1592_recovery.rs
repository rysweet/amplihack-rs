//! Issue #1592: a `profile rotate` swap stopped at every step, then
//! recovered by the next run. Uses the debug-only abort points
//! (`AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER=rotate-<step>`, exit 86). After
//! recovery each login is in exactly one place, by inode, and no refresh
//! lock amplihack made is left behind.

#![cfg(target_os = "linux")]

#[path = "issue_1592_support/mod.rs"]
mod support;

use std::fs;
use support::*;

const LOCK: &str = ".oauth_refresh.lock";

fn abort_at(f: &Fixture, step: &str) {
    let out = f
        .cmd(&["profile", "rotate"])
        .env(ABORT_ENV, format!("rotate-{step}"))
        .output()
        .unwrap();
    assert_no_secrets(&out);
    assert_eq!(
        out.status.code(),
        Some(86),
        "abort at rotate-{step}: {}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    f.assert_each_login_once();
    assert!(
        f.rotate_dir().join("journal.json").exists(),
        "journal left at {step}"
    );
}

/// Usage that needs no swap from either side, so the recovering run only
/// recovers.
fn quiet(f: &Fixture) {
    for p in PROFILES {
        f.usage(p, 10, 10);
    }
}

fn assert_no_amplihack_locks(f: &Fixture) {
    assert!(
        !f.home.join(".claude").join(LOCK).exists(),
        "~/.claude lock left"
    );
    assert!(!f.root.join("spare").join(LOCK).exists(), "spare lock left");
}

#[test]
fn stopped_after_j0_rolls_back() {
    let f = Fixture::new();
    abort_at(&f, "J0");
    let out = f.run(&["profile", "rotate", "--dry-run"]);
    assert!(
        text(&out.stdout).contains("would recover an interrupted swap (phase j0)"),
        "{}",
        text(&out.stdout)
    );
    let status = f.ok(&["profile", "rotate", "status"]);
    assert!(text(&status.stdout).contains("journal: pending (phase j0)"));

    quiet(&f);
    let out = f.ok(&["profile", "rotate"]);
    assert!(
        text(&out.stdout).contains("recovery: rolled back an interrupted swap work->spare"),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    assert_eq!(ino(&f.slot("spare")), Some(f.spare_ino));
    assert_eq!(ino(&f.slot("work")), None);
    assert_eq!(f.home_account(), "uuid-work");
    assert!(!f.rotate_dir().join("journal.json").exists());
    assert_no_amplihack_locks(&f);
    f.assert_each_login_once();
}

#[test]
fn stopped_after_the_exchange_or_later_rolls_forward() {
    for step in ["S1", "S2", "S3", "S4"] {
        let f = Fixture::new();
        abort_at(&f, step);
        quiet(&f);
        let out = f.ok(&["profile", "rotate"]);
        assert!(
            text(&out.stdout).contains("recovery: finished an interrupted swap work->spare"),
            "{step}: {}",
            text(&out.stdout)
        );
        assert_eq!(ino(&f.home_login()), Some(f.spare_ino), "{step}");
        assert_eq!(ino(&f.slot("work")), Some(f.live_ino), "{step}");
        assert_eq!(ino(&f.slot("spare")), None, "{step}");
        assert_eq!(f.home_account(), "uuid-spare", "{step}");
        let ledger = f.ledger().unwrap();
        assert_eq!(ledger["live"], "spare", "{step}");
        assert_eq!(ledger["previous"], "work", "{step}");
        assert!(!f.rotate_dir().join("journal.json").exists(), "{step}");
        assert_no_amplihack_locks(&f);
        f.assert_each_login_once();
    }
}

#[test]
fn a_token_refresh_in_home_after_the_exchange_is_tolerated() {
    let f = Fixture::new();
    abort_at(&f, "S1");
    // Claude refreshed the token in ~/.claude: a new file, a new inode.
    fs::rename(f.home_login(), f.tmp.path().join("old-token")).unwrap();
    let refreshed = login(&f.home_login());
    fs::remove_file(f.tmp.path().join("old-token")).unwrap();
    quiet(&f);
    let out = f.ok(&["profile", "rotate"]);
    assert!(text(&out.stdout).contains("recovery: finished an interrupted swap"));
    assert_eq!(ino(&f.home_login()), Some(refreshed));
    assert_eq!(ino(&f.slot("work")), Some(f.live_ino));
}

#[test]
fn an_unrecognised_layout_is_inconsistent_until_a_person_resolves_it() {
    let f = Fixture::new();
    abort_at(&f, "S1");
    // A person put a login back by hand at the source: neither way is safe.
    let stray = login(&f.slot("work"));
    let before = f.logins();

    let out = f.run(&["profile", "rotate"]);
    assert_eq!(out.status.code(), Some(1), "RotateInconsistent");
    assert!(
        text(&out.stderr)
            .contains("see the Recovery section of docs/howto/claude-profile-rotation.md")
    );
    assert!(
        f.log().contains("ALERT: rotate recovery is inconsistent: "),
        "{}",
        f.log()
    );
    assert_eq!(f.logins(), before, "recovery moved or deleted a login");
    assert_eq!(ino(&f.slot("work")), Some(stray));
    let j: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(f.rotate_dir().join("journal.json")).unwrap())
            .unwrap();
    assert_eq!(j["phase"], "inconsistent");

    let status = f.ok(&["profile", "rotate", "status"]);
    assert!(text(&status.stdout).contains("journal: inconsistent"));

    // Still inconsistent on the next run, even once the stray login is gone.
    fs::rename(f.slot("work"), f.tmp.path().join("aside")).unwrap();
    assert_eq!(f.run(&["profile", "rotate"]).status.code(), Some(1));
}

#[test]
fn a_damaged_journal_is_reported_and_left_unchanged() {
    let f = Fixture::new();
    abort_at(&f, "J0");
    let path = f.rotate_dir().join("journal.json");
    fs::write(&path, r#"{"version":2}"#).unwrap();
    let before = f.logins();
    let out = f.run(&["profile", "rotate"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"version":2}"#);
    assert_eq!(f.logins(), before);
}
