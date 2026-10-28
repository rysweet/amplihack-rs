//! `rotate::ledger`: the rotate ledger and journal under
//! `<root>/.auto-switch.d/rotate/`, and ALERT de-duplication (issue #1592).
//!
//! Contract:
//! - `RotateState::new(root: &Path) -> RotateState` with path fields `dir`,
//!   `ledger`, `journal`, `pid`, `log`, `log1`; `ensure()` creates `dir`
//!   (and `.auto-switch.d`) private.
//! - `RotateLedger { version, live, account, since, swapped_at, previous,
//!   alert, last_cycle: Option<LastCycle { at, measured_at, outcome }> }`,
//!   `Default`
//!   with `version: 1`.
//! - `RotateJournal { version, from, to, adopt, phase: Phase, home,
//!   live_ino, target_ino, home_lock_ino, to_lock_ino, started_at, pid }`.
//! - `load_ledger() -> Result<Option<RotateLedger>, ProfileError>`,
//!   `save_ledger`, `load_journal`, `save_journal`, `delete_journal`.
//! - `alert_key(kind, live: Option<&ProfileName>) -> String`,
//!   `should_alert(&mut RotateLedger, key) -> bool`, `clear_alert(&mut ..)`.

use super::ledger::{
    LastCycle, Phase, RotateJournal, RotateLedger, RotateState, alert_key, clear_alert,
    for_older_readers, should_alert,
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
            measured_at: Some(130),
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
fn unknown_versions_fail_closed_and_leave_the_file() {
    let r = root();
    for body in [
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

/// Issue #1659: the limit-resume hook reads the ledger the rotator writes,
/// through `amplihack_utils::rotator_status`, and judges freshness by
/// `measured_at`.
#[test]
fn the_hooks_reader_sees_when_the_cycle_measured() {
    let r = root();
    r.rs.save_ledger(&ledger()).unwrap();
    let status = amplihack_utils::rotator_status::read_cycle(&r.rs.root, 200).unwrap();
    assert_eq!((status.at, status.measured_at), (Some(160), Some(130)));
    assert!(status.fresh_capacity(129), "measured after a limit at 129");
    assert!(
        !status.fresh_capacity(130),
        "measured before a limit at 130"
    );
}

#[test]
fn a_cycle_that_measured_nothing_writes_no_measured_at() {
    let r = root();
    let mut l = ledger();
    l.last_cycle = Some(LastCycle {
        at: 160,
        measured_at: None,
        outcome: "alerted".into(),
    });
    r.rs.save_ledger(&l).unwrap();
    let raw = fs::read_to_string(&r.rs.ledger).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(json["last_cycle"].get("measured_at").is_none(), "{raw}");
    assert_eq!(r.rs.load_ledger().unwrap(), Some(l));
}

/// Issue #1659: a field this binary does not know, at the top or in
/// `last_cycle`, is ignored, so a newer amplihack's ledger never makes this
/// rotator report an inconsistency.
#[test]
fn unknown_ledger_fields_are_ignored() {
    let r = root();
    let body = r#"{"version":1,"live":"spare","surprise":true,
        "last_cycle":{"at":5,"outcome":"stayed","from_the_future":[1]}}"#;
    fs::write(&r.rs.ledger, body).unwrap();
    fs::set_permissions(&r.rs.ledger, fs::Permissions::from_mode(0o600)).unwrap();
    let l = r.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.live.as_deref(), Some("spare"));
    assert_eq!(l.last_cycle.unwrap().outcome, "stayed");
}

/// The ledger as amplihack before #1659 read it: unknown fields rejected.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct OldLedger {
    version: u32,
    live: Option<String>,
    account: Option<String>,
    since: Option<i64>,
    swapped_at: Option<i64>,
    previous: Option<String>,
    alert: Option<String>,
    last_cycle: Option<OldLastCycle>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct OldLastCycle {
    at: i64,
    outcome: String,
}

/// Issue #1659: while a rotator from an older binary runs, what this binary
/// writes is a ledger that rotator parses, so a one-shot `rotate` beside it
/// cannot start an "inconsistent" ALERT loop. Without the downgrade it would.
///
/// The first assertion pins the other direction too: a ledger written with
/// no older rotator running is one an older binary cannot read, which is why
/// going back to it needs `rotate stop` and the ledger deleted first (the
/// "Go back to an older amplihack" section of
/// docs/howto/claude-profile-rotation.md).
#[test]
fn a_ledger_written_beside_an_older_rotator_is_one_it_can_read() {
    let full = serde_json::to_vec(&for_older_readers(&ledger(), false)).unwrap();
    assert!(
        serde_json::from_slice::<OldLedger>(&full).is_err(),
        "an older reader must reject measured_at, or the rollback steps in the docs are wrong"
    );
    let older = serde_json::to_vec(&for_older_readers(&ledger(), true)).unwrap();
    serde_json::from_slice::<OldLedger>(&older).expect("the old reader accepts it");
}

// --- Issue #1693: `guidance` and `lost_login` -------------------------------
//
// Contract: `RotateLedger.guidance: Option<Vec<String>>` (at most 16 lines,
// each at most 1024 bytes, no control or bidirectional-override
// characters) and `RotateLedger.lost_login: Vec<String>` (at most 64
// distinct valid profile names). Both are omitted when empty, checked on
// load (a failure is `RotateInconsistent`) and left out for older readers.

fn with_handfix() -> RotateLedger {
    RotateLedger {
        guidance: Some(vec![
            "ALERT: the account in ~/.claude is not the one the ledger names (live=work); no profile has that account; no swap".into(),
            "fix 1: amplihack profile login work".into(),
        ]),
        lost_login: vec!["work".into()],
        home_unreadable_since: Some(1_700_000_000),
        ..ledger()
    }
}

fn write_raw(r: &Root, v: &serde_json::Value) {
    fs::write(&r.rs.ledger, serde_json::to_vec(v).unwrap()).unwrap();
    fs::set_permissions(&r.rs.ledger, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn guidance_and_lost_login_round_trip() {
    let r = root();
    let l = with_handfix();
    r.rs.save_ledger(&l).unwrap();
    assert_eq!(r.rs.load_ledger().unwrap(), Some(l));
}

#[test]
fn a_ledger_without_the_new_fields_loads_and_writes_none() {
    let r = root();
    write_raw(&r, &serde_json::json!({"version": 1, "live": "spare"}));
    let l = r.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.guidance, None);
    assert_eq!(l.home_unreadable_since, None);
    assert!(l.lost_login.is_empty());
    r.rs.save_ledger(&l).unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&r.rs.ledger).unwrap()).unwrap();
    assert!(raw.get("guidance").is_none(), "{raw}");
    assert!(raw.get("lost_login").is_none(), "{raw}");
    assert!(raw.get("home_unreadable_since").is_none(), "{raw}");
    assert_eq!(RotateLedger::default().guidance, None);
    assert!(RotateLedger::default().lost_login.is_empty());
}

#[test]
fn older_readers_get_a_ledger_without_the_new_fields() {
    let older = for_older_readers(&with_handfix(), true);
    assert_eq!(older.guidance, None);
    assert_eq!(older.home_unreadable_since, None);
    assert!(older.lost_login.is_empty());
    serde_json::from_slice::<OldLedger>(&serde_json::to_vec(&older).unwrap())
        .expect("the old reader accepts it");
    assert_eq!(for_older_readers(&with_handfix(), false), with_handfix());
}

#[test]
fn guidance_at_the_limits_loads() {
    let r = root();
    let mut v = serde_json::to_value(ledger()).unwrap();
    v["guidance"] = serde_json::json!(vec!["x".repeat(1024); 16]);
    v["lost_login"] = serde_json::json!((0..64).map(|i| format!("p{i}")).collect::<Vec<_>>());
    write_raw(&r, &v);
    let l = r.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.guidance.unwrap().len(), 16);
    assert_eq!(l.lost_login.len(), 64);
}

#[test]
fn guidance_or_lost_login_outside_the_limits_fails_closed() {
    let r = root();
    let base = serde_json::to_value(ledger()).unwrap();
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("guidance", serde_json::json!(vec!["x"; 17])),
        ("guidance", serde_json::json!(["x".repeat(1025)])),
        ("guidance", serde_json::json!(["bell \u{7}"])),
        ("guidance", serde_json::json!(["escape \u{1b}[2J"])),
        ("guidance", serde_json::json!(["newline\nfix 9: rm -rf ~"])),
        ("guidance", serde_json::json!(["override \u{202e}"])),
        ("guidance", serde_json::json!(["isolate \u{2066}"])),
        ("lost_login", serde_json::json!(["../escape"])),
        ("lost_login", serde_json::json!(["work", "work"])),
        ("lost_login", serde_json::json!([""])),
        (
            "lost_login",
            serde_json::json!((0..65).map(|i| format!("p{i}")).collect::<Vec<_>>()),
        ),
    ];
    for (key, value) in cases {
        let mut v = base.clone();
        v[key] = value;
        write_raw(&r, &v);
        let before = fs::read(&r.rs.ledger).unwrap();
        inconsistent(r.rs.load_ledger());
        assert_eq!(fs::read(&r.rs.ledger).unwrap(), before, "left as it was");
    }
}
