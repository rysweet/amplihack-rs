//! `usage_passive` (issue #1844): a fresh stored reading in `usage.json`
//! stands in for a probe.
//!
//! Contract:
//! - `usage_passive::fresh(root: &Path, dir: &ProfileName, max_age: u64,
//!   now: i64) -> Option<UsageReport>`.
//! - It looks up the account whose login is in `dir` (the auto-switch
//!   ledger, `account_in`), then that account's `usage.json` entry. It is
//!   `None` when auto-switch state does not exist, there is no entry,
//!   `observed_at` is in the future or more than `max_age` seconds ago, or
//!   no window passes `auto_switch::usage::plausible`. `max_age` 0 is always
//!   `None`.
//! - The report: `five_hour` is `session` (label `session`), `seven_day` is
//!   `week_all` (label `all models`), `resets_text` is the reset as
//!   `YYYY-MM-DDTHH:MM:SSZ`, `week_models` is empty and `session_limit` is
//!   `true`. The state follows the usual rule: a window at 100% is
//!   exhausted, session first.
//! - An implausible window is left out; the plausible one is still used.
//!
//! Every test uses a temporary profile root.

use super::name::ProfileName;
use super::usage_parse::UsageState;
use super::usage_passive::fresh;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// 2026-11-01T12:00:00Z.
const NOW: i64 = 1_793_534_400;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

fn name(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

struct Root {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Root {
    fn state(&self) -> PathBuf {
        self.root.join(".auto-switch.d")
    }

    fn write(&self, file: &str, v: &Value) {
        let dir = self.state();
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let p = dir.join(file);
        fs::write(&p, v.to_string()).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn usage(&self, accounts: Value) {
        self.write("usage.json", &json!({"version": 1, "accounts": accounts}));
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

fn root() -> Root {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = tmp.path().join("profiles");
    for p in ["work", "spare"] {
        fs::create_dir_all(root.join(p)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let root = root.canonicalize().unwrap();
    Root { _tmp: tmp, root }
}

fn reading(session: Option<f64>, week: Option<f64>, observed_at: i64) -> Value {
    let mut r = serde_json::Map::new();
    if let Some(s) = session {
        r.insert(
            "five_hour".into(),
            json!({"used_percentage": s, "resets_at": NOW + 2 * HOUR}),
        );
    }
    if let Some(w) = week {
        r.insert(
            "seven_day".into(),
            json!({"used_percentage": w, "resets_at": NOW + 3 * DAY}),
        );
    }
    r.insert("observed_at".into(), json!(observed_at));
    r.insert("source".into(), json!("statusline"));
    Value::Object(r)
}

#[test]
fn a_fresh_reading_becomes_the_report() {
    let r = root();
    r.usage(json!({"spare": reading(Some(20.0), Some(55.0), NOW - 10)}));
    let rep = fresh(r.path(), &name("spare"), 300, NOW).expect("fresh");
    assert_eq!(rep.state, UsageState::Ok);
    let s = rep.session.expect("session");
    assert_eq!(s.label, "session");
    assert_eq!(s.used_percentage, 20.0);
    assert_eq!(s.resets_at, Some(NOW + 2 * HOUR));
    assert_eq!(s.resets_text.as_deref(), Some("2026-11-01T14:00:00Z"));
    let w = rep.week_all.expect("week");
    assert_eq!(w.label, "all models");
    assert_eq!(w.used_percentage, 55.0);
    assert_eq!(w.resets_text.as_deref(), Some("2026-11-04T12:00:00Z"));
    assert!(rep.week_models.is_empty());
    assert!(rep.session_limit);
}

#[test]
fn an_exhausted_session_keeps_the_weekly_figure() {
    let r = root();
    r.usage(json!({"spare": reading(Some(100.0), Some(55.0), NOW - 10)}));
    let rep = fresh(r.path(), &name("spare"), 300, NOW).unwrap();
    assert_eq!(rep.state, UsageState::SessionExhausted);
    assert_eq!(rep.week_all.map(|w| w.used_percentage), Some(55.0));
}

#[test]
fn an_exhausted_week_is_weekly_exhausted() {
    let r = root();
    r.usage(json!({"spare": reading(Some(10.0), Some(100.0), NOW - 10)}));
    let rep = fresh(r.path(), &name("spare"), 300, NOW).unwrap();
    assert_eq!(rep.state, UsageState::WeeklyExhausted);
}

#[test]
fn the_age_limit_is_inclusive_and_a_stale_reading_is_none() {
    let r = root();
    r.usage(json!({"spare": reading(Some(1.0), Some(2.0), NOW - 300)}));
    assert!(fresh(r.path(), &name("spare"), 300, NOW).is_some());
    r.usage(json!({"spare": reading(Some(1.0), Some(2.0), NOW - 301)}));
    assert!(fresh(r.path(), &name("spare"), 300, NOW).is_none());
}

#[test]
fn a_reading_from_the_future_is_none() {
    let r = root();
    r.usage(json!({"spare": reading(Some(1.0), Some(2.0), NOW + 5)}));
    assert!(fresh(r.path(), &name("spare"), 300, NOW).is_none());
}

#[test]
fn max_age_zero_turns_passive_readings_off() {
    let r = root();
    r.usage(json!({"spare": reading(Some(1.0), Some(2.0), NOW)}));
    assert!(fresh(r.path(), &name("spare"), 0, NOW).is_none());
}

#[test]
fn no_state_or_no_entry_is_none() {
    let r = root();
    assert!(fresh(r.path(), &name("spare"), 300, NOW).is_none());
    r.usage(json!({"work": reading(Some(1.0), Some(2.0), NOW - 1)}));
    assert!(fresh(r.path(), &name("spare"), 300, NOW).is_none());
}

#[test]
fn a_damaged_usage_file_is_none() {
    let r = root();
    r.write("usage.json", &json!("not the format"));
    assert!(fresh(r.path(), &name("spare"), 300, NOW).is_none());
}

#[test]
fn implausible_windows_are_left_out() {
    let r = root();
    let mut entry = reading(Some(30.0), Some(40.0), NOW - 10);
    entry["seven_day"]["resets_at"] = json!(NOW + 30 * DAY);
    r.usage(json!({"spare": entry}));
    let rep = fresh(r.path(), &name("spare"), 300, NOW).expect("session is plausible");
    assert_eq!(rep.session.map(|w| w.used_percentage), Some(30.0));
    assert_eq!(rep.week_all, None);

    let mut entry = reading(Some(30.0), Some(40.0), NOW - 10);
    entry["seven_day"]["resets_at"] = json!(NOW + 30 * DAY);
    entry["five_hour"]["resets_at"] = json!(NOW - 2 * DAY);
    r.usage(json!({"spare": entry}));
    assert!(fresh(r.path(), &name("spare"), 300, NOW).is_none());
}

#[test]
fn no_session_window_reads_with_the_session_limit_kept() {
    let r = root();
    r.usage(json!({"spare": reading(None, Some(40.0), NOW - 10)}));
    let rep = fresh(r.path(), &name("spare"), 300, NOW).unwrap();
    assert_eq!(rep.session, None);
    assert!(
        rep.session_limit,
        "usage.json does not record no session limit"
    );
    assert_eq!(rep.state, UsageState::Ok);
}

#[test]
fn the_account_in_the_directory_is_the_one_read() {
    // `work`'s login is lent to `spare`'s directory: `spare` reads work's
    // entry, never its own.
    let r = root();
    r.write(
        "ledger.json",
        &json!({"version": 1, "hosts": {
            "spare": {"live": "work", "swapped_at": NOW - DAY, "pre_swap_fingerprint": null}
        }}),
    );
    r.usage(json!({
        "work": reading(Some(70.0), Some(80.0), NOW - 10),
        "spare": reading(Some(1.0), Some(2.0), NOW - 10),
    }));
    let rep = fresh(r.path(), &name("spare"), 300, NOW).unwrap();
    assert_eq!(rep.session.map(|w| w.used_percentage), Some(70.0));
    assert_eq!(rep.week_all.map(|w| w.used_percentage), Some(80.0));
}
