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
    LastCycle, OverflowBlock, Phase, RotateJournal, RotateLedger, RotateState, active_blocks,
    alert_key, clear_alert, prune_blocks, should_alert, spend_reset,
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
            overflow: None,
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

// --- Overflow fields (issue #1606) -----------------------------------------
//
// Contract:
// - `LastCycle.overflow: Option<String>` ("in" | "out" | "on"), after
//   `outcome`, omitted when `None`.
// - `RotateLedger.overflow_blocked: BTreeMap<String, OverflowBlock>`, last,
//   omitted when empty. `OverflowBlock { at, until, reason }` ignores unknown
//   fields (D5); `LastCycle` and the top level stay strict.
// - `load_ledger` checks: each key is a profile name; `reason` is
//   `spend_limit`; `0 < at < until <= at + 62 days`, computed with checked
//   arithmetic so an overflowing `at` is invalid rather than a panic (D4); at
//   most 256 entries; `overflow` is one of in/out/on. A failure is
//   `RotateInconsistent` naming the field, never echoing the value.
// - `spend_reset(at)`: the 1st of the next calendar month, 00:00 UTC. It is
//   the `until` the writer uses, not one the reader insists on.
// - `active_blocks(&l, now)`: the blocks with `now < until`.
// - `prune_blocks(&mut l, now)`: drops the blocks with `until <= now`.

fn utc(y: i32, m: u32, d: u32, h: u32, mi: u32, sec: u32) -> i64 {
    use chrono::TimeZone;
    chrono::Utc
        .with_ymd_and_hms(y, m, d, h, mi, sec)
        .single()
        .unwrap()
        .timestamp()
}

fn ob(at: i64) -> OverflowBlock {
    OverflowBlock {
        at,
        until: spend_reset(at),
        reason: "spend_limit".into(),
    }
}

const DAY_62: i64 = 62 * 86_400;

fn write_raw(r: &Root, body: &str) {
    fs::write(&r.rs.ledger, body).unwrap();
    fs::set_permissions(&r.rs.ledger, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn a_ledger_without_overflow_is_byte_identical_to_before() {
    let raw = String::from_utf8(serde_json::to_vec_pretty(&ledger()).unwrap()).unwrap();
    assert_eq!(
        raw,
        r#"{
  "version": 1,
  "live": "spare",
  "account": "uuid-spare",
  "since": 100,
  "swapped_at": 100,
  "previous": "work",
  "last_cycle": {
    "at": 160,
    "outcome": "stayed"
  }
}"#
    );
}

#[test]
fn overflow_fields_round_trip_and_sit_after_outcome_and_last() {
    let r = root();
    let at = utc(2026, 10, 26, 12, 0, 0);
    let mut l = ledger();
    l.last_cycle.as_mut().unwrap().overflow = Some("in".into());
    l.overflow_blocked.insert("cred".into(), ob(at));
    r.rs.save_ledger(&l).unwrap();
    assert_eq!(r.rs.load_ledger().unwrap(), Some(l));
    let raw = fs::read_to_string(&r.rs.ledger).unwrap();
    let pos = |k: &str| {
        raw.find(&format!("\"{k}\""))
            .unwrap_or_else(|| panic!("{k}: {raw}"))
    };
    assert!(pos("outcome") < pos("overflow"), "{raw}");
    assert!(pos("last_cycle") < pos("overflow_blocked"), "{raw}");
}

#[test]
fn spend_reset_is_the_first_of_next_month_utc() {
    assert_eq!(
        spend_reset(utc(2026, 10, 26, 12, 0, 0)),
        utc(2026, 11, 1, 0, 0, 0)
    );
    assert_eq!(
        spend_reset(utc(2026, 12, 31, 23, 59, 59)),
        utc(2027, 1, 1, 0, 0, 0),
        "December rolls into January"
    );
    assert_eq!(
        spend_reset(utc(2026, 11, 1, 0, 0, 0)),
        utc(2026, 12, 1, 0, 0, 0),
        "at the 1st 00:00 the block lasts a whole month"
    );
    assert_eq!(
        spend_reset(utc(2028, 2, 29, 8, 0, 0)),
        utc(2028, 3, 1, 0, 0, 0),
        "leap day"
    );
}

#[test]
fn blocks_are_active_until_their_end_and_pruned_at_it() {
    let at = utc(2026, 10, 26, 12, 0, 0);
    let until = spend_reset(at);
    let mut l = RotateLedger::default();
    l.overflow_blocked.insert("cred".into(), ob(at));
    assert!(active_blocks(&l, until - 1).contains_key("cred"));
    assert!(
        !active_blocks(&l, until).contains_key("cred"),
        "until is exclusive"
    );
    prune_blocks(&mut l, until - 1);
    assert_eq!(l.overflow_blocked.len(), 1);
    prune_blocks(&mut l, until);
    assert!(l.overflow_blocked.is_empty());
}

#[test]
fn invalid_overflow_fields_fail_closed_without_echoing_the_value() {
    let r = root();
    let at = utc(2026, 10, 26, 12, 0, 0);
    let good_until = spend_reset(at);
    let blocked = |key: &str, at: i64, until: i64, reason: &str| {
        serde_json::json!({
            "version": 1,
            "overflow_blocked": { key: { "at": at, "until": until, "reason": reason } }
        })
        .to_string()
    };
    let bad = [
        blocked("../evil", at, good_until, "spend_limit"),
        blocked("cred", at, good_until, "SECRET-REASON"),
        blocked("cred", 0, spend_reset(0), "spend_limit"),
        blocked("cred", -5, spend_reset(-5), "spend_limit"),
        blocked("cred", at, i64::MAX, "spend_limit"),
        blocked("cred", at, at, "spend_limit"),
        blocked("cred", at, at - 1, "spend_limit"),
        blocked("cred", at, at + DAY_62 + 1, "spend_limit"),
        blocked("cred", i64::MAX - 1, i64::MAX, "spend_limit"),
        serde_json::json!({
            "version": 1,
            "last_cycle": { "at": 5, "outcome": "stayed", "overflow": "SECRET-TAG" }
        })
        .to_string(),
    ];
    for body in bad {
        write_raw(&r, &body);
        let err = r.rs.load_ledger().unwrap_err();
        assert!(
            matches!(err, ProfileError::RotateInconsistent(_)),
            "{err:?}"
        );
        let msg = err.to_string();
        for secret in ["SECRET", "evil"] {
            assert!(!msg.contains(secret), "{msg}");
        }
        assert_eq!(
            fs::read_to_string(&r.rs.ledger).unwrap(),
            body,
            "left alone"
        );
    }
}

#[test]
fn every_known_overflow_tag_loads() {
    let r = root();
    for tag in ["in", "out", "on"] {
        let body = serde_json::json!({
            "version": 1,
            "last_cycle": { "at": 5, "outcome": "stayed", "overflow": tag }
        });
        write_raw(&r, &body.to_string());
        let l = r.rs.load_ledger().unwrap().unwrap();
        assert_eq!(l.last_cycle.unwrap().overflow.as_deref(), Some(tag));
    }
}

#[test]
fn more_than_256_blocks_are_refused() {
    let r = root();
    let at = utc(2026, 10, 26, 12, 0, 0);
    let mut l = RotateLedger::default();
    for i in 0..256 {
        l.overflow_blocked.insert(format!("p{i}"), ob(at));
    }
    write_raw(&r, &serde_json::to_string(&l).unwrap());
    assert_eq!(
        r.rs.load_ledger().unwrap().unwrap().overflow_blocked.len(),
        256
    );
    l.overflow_blocked.insert("p256".into(), ob(at));
    write_raw(&r, &serde_json::to_string(&l).unwrap());
    inconsistent(r.rs.load_ledger());
}

// --- Relaxed block validation (D4, D5) -------------------------------------

fn block_body(at: i64, until: i64) -> String {
    serde_json::json!({
        "version": 1,
        "overflow_blocked": { "cred": { "at": at, "until": until, "reason": "spend_limit" } }
    })
    .to_string()
}

#[test]
fn a_block_ending_before_the_spend_reset_loads() {
    // A later change may take the reset time from Claude's message; a block
    // that ends three days after it began must not make the ledger invalid.
    let r = root();
    let at = utc(2026, 10, 26, 12, 0, 0);
    write_raw(&r, &block_body(at, at + 3 * 86_400));
    let l = r.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.overflow_blocked["cred"].until, at + 3 * 86_400);
}

#[test]
fn a_block_may_last_exactly_62_days_and_no_longer() {
    let r = root();
    let at = utc(2026, 10, 26, 12, 0, 0);
    write_raw(&r, &block_body(at, at + DAY_62));
    assert!(
        r.rs.load_ledger().unwrap().is_some(),
        "the bound is inclusive"
    );
    write_raw(&r, &block_body(at, at + DAY_62 + 1));
    inconsistent(r.rs.load_ledger());
}

#[test]
fn a_block_whose_end_is_not_after_its_start_is_refused() {
    let r = root();
    let at = utc(2026, 10, 26, 12, 0, 0);
    for until in [at, at - 1, 0] {
        write_raw(&r, &block_body(at, until));
        inconsistent(r.rs.load_ledger());
    }
}

#[test]
fn a_block_near_i64_max_is_refused_without_panicking() {
    let r = root();
    for (at, until) in [(i64::MAX - 1, i64::MAX), (i64::MAX, i64::MAX)] {
        // `at + 62 days` overflows here; it must be refused, not panic.
        write_raw(&r, &block_body(at, until));
        inconsistent(r.rs.load_ledger());
    }
}

#[test]
fn an_unknown_field_inside_a_block_is_ignored() {
    let r = root();
    let at = utc(2026, 10, 26, 12, 0, 0);
    let body = serde_json::json!({
        "version": 1,
        "overflow_blocked": { "cred": { "at": at, "until": spend_reset(at),
                                        "reason": "spend_limit", "source": "message" } }
    })
    .to_string();
    write_raw(&r, &body);
    let l = r.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.overflow_blocked["cred"], ob(at));
}

#[test]
fn unknown_fields_outside_a_block_stay_refused() {
    // #1687 owns the tolerance of `last_cycle` and the top level.
    let r = root();
    for body in [
        serde_json::json!({ "version": 1, "future": 1 }),
        serde_json::json!({
            "version": 1,
            "last_cycle": { "at": 5, "outcome": "stayed", "future": 1 }
        }),
    ] {
        write_raw(&r, &body.to_string());
        inconsistent(r.rs.load_ledger());
    }
}

#[test]
fn a_saved_ledger_without_overflow_data_writes_no_overflow_keys() {
    let r = root();
    r.rs.save_ledger(&ledger()).unwrap();
    let raw = fs::read_to_string(&r.rs.ledger).unwrap();
    assert!(!raw.contains("overflow"), "{raw}");
}
