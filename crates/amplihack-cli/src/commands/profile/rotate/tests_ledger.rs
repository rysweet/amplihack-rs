//! `rotate::ledger`: the rotate ledger and journal under
//! `<root>/.auto-switch.d/rotate/`, and ALERT de-duplication (issue #1592).
//!
//! Contract:
//! - `RotateState::new(root: &Path) -> RotateState` with path fields `dir`,
//!   `ledger`, `journal`, `pid`, `log`, `log1`; `ensure()` creates `dir`
//!   (and `.auto-switch.d`) private.
//! - `RotateLedger { version, live, account, since, swapped_at, previous,
//!   alert, last_cycle: Option<LastCycle { at, outcome }> }`, `Default`
//!   with `version: 1`.
//! - `RotateJournal { version, from, to, adopt, phase: Phase, home,
//!   live_ino, target_ino, home_lock_ino, to_lock_ino, started_at, pid }`.
//! - `load_ledger() -> Result<Option<RotateLedger>, ProfileError>`,
//!   `save_ledger`, `load_journal`, `save_journal`, `delete_journal`.
//! - `alert_key(kind, live: Option<&ProfileName>) -> String`,
//!   `should_alert(&mut RotateLedger, key) -> bool`, `clear_alert(&mut ..)`.

use super::ledger::{
    LastCycle, Phase, RotateJournal, RotateLedger, RotateState, alert_key, clear_alert,
    should_alert,
};
use crate::commands::profile::ProfileError;
use crate::commands::profile::name::ProfileName;
use std::fs;
use std::os::unix::fs::PermissionsExt;

struct Root {
    _tmp: tempfile::TempDir,
    rs: RotateState,
}

fn root() -> Root {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let rs = RotateState::new(tmp.path());
    rs.ensure().unwrap();
    Root { _tmp: tmp, rs }
}

fn ledger() -> RotateLedger {
    RotateLedger {
        live: Some("spare".into()),
        account: Some("uuid-spare".into()),
        since: Some(100),
        swapped_at: Some(100),
        previous: Some("work".into()),
        last_cycle: Some(LastCycle {
            at: 160,
            outcome: "stayed".into(),
        }),
        ..RotateLedger::default()
    }
}

fn journal() -> RotateJournal {
    RotateJournal {
        version: 1,
        from: "work".into(),
        to: "spare".into(),
        adopt: false,
        phase: Phase::J0,
        home: "/home/someone".into(),
        live_ino: 11,
        target_ino: 22,
        home_lock_ino: None,
        to_lock_ino: None,
        started_at: 1,
        pid: 2,
    }
}

fn inconsistent<T: std::fmt::Debug>(r: Result<T, ProfileError>) {
    assert!(
        matches!(r, Err(ProfileError::RotateInconsistent(_))),
        "expected RotateInconsistent, got {r:?}"
    );
}

#[test]
fn state_paths_and_modes() {
    let r = root();
    assert!(r.rs.dir.ends_with(".auto-switch.d/rotate"));
    for (p, n) in [
        (&r.rs.ledger, "ledger.json"),
        (&r.rs.journal, "journal.json"),
        (&r.rs.pid, "rotate.pid"),
        (&r.rs.log, "rotate.log"),
        (&r.rs.log1, "rotate.log.1"),
    ] {
        assert_eq!(p.parent().unwrap(), r.rs.dir);
        assert!(p.ends_with(n));
    }
    let mode = fs::metadata(&r.rs.dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn ledger_round_trips_privately_and_omits_unset_fields() {
    let r = root();
    assert_eq!(r.rs.load_ledger().unwrap(), None);
    let l = RotateLedger {
        previous: None,
        alert: None,
        ..ledger()
    };
    r.rs.save_ledger(&l).unwrap();
    assert_eq!(r.rs.load_ledger().unwrap(), Some(l));
    let text = fs::read_to_string(&r.rs.ledger).unwrap();
    assert!(
        !text.contains("previous") && !text.contains("alert"),
        "{text}"
    );
    assert!(text.contains("\"version\": 1") || text.contains("\"version\":1"));
    let mode = fs::metadata(&r.rs.ledger).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn journal_round_trips_with_lower_case_phases() {
    let r = root();
    assert_eq!(r.rs.load_journal().unwrap(), None);
    for (phase, text) in [
        (Phase::J0, "j0"),
        (Phase::S1, "s1"),
        (Phase::S2, "s2"),
        (Phase::S3, "s3"),
        (Phase::S4, "s4"),
        (Phase::Inconsistent, "inconsistent"),
    ] {
        let j = RotateJournal {
            phase,
            home_lock_ino: Some(5),
            ..journal()
        };
        r.rs.save_journal(&j).unwrap();
        let raw = fs::read_to_string(&r.rs.journal).unwrap();
        assert!(raw.contains(&format!("\"{text}\"")), "{raw}");
        assert!(!raw.contains("to_lock_ino"), "unset lock omitted: {raw}");
        assert_eq!(r.rs.load_journal().unwrap(), Some(j));
    }
    r.rs.delete_journal().unwrap();
    assert_eq!(r.rs.load_journal().unwrap(), None);
    r.rs.delete_journal().unwrap();
}

#[test]
fn unknown_fields_and_versions_fail_closed_and_leave_the_file() {
    let r = root();
    for body in [
        r#"{"version":1,"live":"spare","surprise":true}"#,
        r#"{"version":2,"live":"spare"}"#,
        r#"{"live":"spare"}"#,
        "not json",
        "",
    ] {
        fs::write(&r.rs.ledger, body).unwrap();
        fs::set_permissions(&r.rs.ledger, fs::Permissions::from_mode(0o600)).unwrap();
        inconsistent(r.rs.load_ledger());
        assert_eq!(fs::read_to_string(&r.rs.ledger).unwrap(), body);
    }
    fs::write(&r.rs.journal, r#"{"version":9}"#).unwrap();
    inconsistent(r.rs.load_journal());
    assert_eq!(
        fs::read_to_string(&r.rs.journal).unwrap(),
        r#"{"version":9}"#
    );
}

#[test]
fn names_read_back_are_checked() {
    let r = root();
    for field in ["live", "previous"] {
        let body = format!(r#"{{"version":1,"{field}":"../x"}}"#);
        fs::write(&r.rs.ledger, &body).unwrap();
        inconsistent(r.rs.load_ledger());
    }
    for (field, bad) in [("from", "../x"), ("to", "a/b")] {
        let mut v = serde_json::to_value(journal()).unwrap();
        v[field] = bad.into();
        fs::write(&r.rs.journal, v.to_string()).unwrap();
        inconsistent(r.rs.load_journal());
    }
}

#[test]
fn an_over_long_or_controlled_account_is_refused() {
    let r = root();
    for account in ["x".repeat(321), "a\u{7}b".into()] {
        let body = serde_json::json!({"version": 1, "live": "spare", "account": account});
        fs::write(&r.rs.ledger, body.to_string()).unwrap();
        inconsistent(r.rs.load_ledger());
    }
    let ok = serde_json::json!({"version": 1, "live": "spare", "account": "x".repeat(320)});
    fs::write(&r.rs.ledger, ok.to_string()).unwrap();
    assert!(r.rs.load_ledger().unwrap().is_some());
}

#[test]
fn files_over_64_kib_are_refused() {
    let r = root();
    let pad = " ".repeat(64 * 1024);
    fs::write(&r.rs.ledger, format!(r#"{{"version":1{pad}}}"#)).unwrap();
    inconsistent(r.rs.load_ledger());
}

#[test]
fn a_symlinked_ledger_is_refused() {
    let r = root();
    let elsewhere = r.rs.dir.join("elsewhere.json");
    fs::write(&elsewhere, r#"{"version":1,"live":"spare"}"#).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &r.rs.ledger).unwrap();
    assert!(r.rs.load_ledger().is_err());
    assert!(
        r.rs.save_ledger(&ledger()).is_err(),
        "never written through"
    );
    assert_eq!(
        fs::read_to_string(&elsewhere).unwrap(),
        r#"{"version":1,"live":"spare"}"#
    );
}

#[test]
fn alert_keys_are_kind_and_live_profile() {
    let work = ProfileName::parse("work").unwrap();
    assert_eq!(alert_key("no-capacity", Some(&work)), "no-capacity:work");
    assert_eq!(alert_key("no-login", None), "no-login:-");
}

#[test]
fn an_alert_is_written_once_until_it_changes_or_clears() {
    let mut l = RotateLedger::default();
    let key = "no-capacity:work";
    assert!(should_alert(&mut l, key), "first time");
    assert!(!should_alert(&mut l, key), "repeat is suppressed");
    assert!(should_alert(&mut l, "no-login:work"), "a new key");
    assert!(should_alert(&mut l, key), "back to the first key");
    clear_alert(&mut l);
    assert_eq!(l.alert, None);
    assert!(should_alert(&mut l, key), "after a clear");
}

#[test]
fn alert_state_persists_through_the_file() {
    let r = root();
    let mut l = ledger();
    assert!(should_alert(&mut l, "no-capacity:spare"));
    r.rs.save_ledger(&l).unwrap();
    let mut again = r.rs.load_ledger().unwrap().unwrap();
    assert!(!should_alert(&mut again, "no-capacity:spare"));
}
