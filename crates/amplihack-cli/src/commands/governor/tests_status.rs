//! Issue #1811 (TDD): `amplihack governor status [--json]`.
//!
//! Wire with `#[cfg(all(test, unix))] mod tests_status;` in `governor/mod.rs`.
//!
//! Expected API in `status`:
//! - `view(root: Option<&Path>, &Settings, manifest: Option<&Path>, now)
//!   -> StatusView` (`Serialize`): read-only; no lock, no write, no reclaim,
//!   no quarantine.
//! - `render_table(&StatusView) -> String`.
//! - `display_safe(&str) -> String`: control characters become `?`, at most
//!   64 characters.
//!
//! JSON schema: docs/reference/capacity-governor.md#json-output.

use super::config::Settings;
use super::model::{Class, Slot};
use super::status::{display_safe, render_table, view};
use super::store::{History, Queue, State, Store};
use super::tests_seed::{NOW, Root, dead_pid};
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::Duration;

fn json(root: Option<&std::path::Path>, settings: &Settings) -> Value {
    serde_json::to_value(view(root, settings, None, NOW)).unwrap()
}

fn keys(v: &Value) -> BTreeSet<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

fn set(names: &[&'static str]) -> BTreeSet<&'static str> {
    names.iter().copied().collect()
}

fn seed_state(r: &Root, slot_pid: u32) {
    let store = Store::open(&r.root).unwrap();
    let lock = store.lock_exclusive(Duration::from_secs(1)).unwrap();
    let mut state = State::default();
    state.slots.push(Slot {
        pid: slot_pid,
        start_time: None,
        workstream: "ws-1811".into(),
        class: Class::Ordinary,
        tree_id: "t-1".into(),
        admitted_at: NOW - 60,
        nested_children: 3,
        start: super::model::Start::Agent,
    });
    store
        .save(&lock, &History::default(), &Queue::default(), &state)
        .unwrap();
}

#[test]
fn top_level_and_nested_key_sets_are_exact() {
    let r = Root::new(&[("alpha", 62.0, 71.0), ("beta", -1.0, 38.0)]);
    seed_state(&r, std::process::id());
    let v = json(Some(&r.root), &Settings::default());
    assert_eq!(
        keys(&v),
        set(&[
            "version",
            "generated_at",
            "enabled",
            "limit",
            "max",
            "profiles",
            "reserve",
            "slots",
            "queue",
            "workstreams",
            "fail_open_reason",
        ])
    );
    assert_eq!(v["version"], 1);
    assert_eq!(v["max"], 4);
    let window = set(&[
        "used_pct",
        "resets_at",
        "burn_rate",
        "sustainable_rate",
        "forecast_exhaustion",
    ]);
    let profile = set(&[
        "name",
        "active",
        "eligible",
        "session_window",
        "invalid_reading",
        "session",
        "weekly",
        "burn_rate",
        "sustainable_rate",
        "forecast_exhaustion",
    ]);
    let profiles = v["profiles"].as_array().unwrap();
    assert_eq!(profiles.len(), 2);
    for p in profiles {
        assert_eq!(keys(p), profile);
        assert_eq!(keys(&p["weekly"]), window);
    }
    assert_eq!(keys(&profiles[0]["session"]), window);
    assert_eq!(
        keys(&v["reserve"]),
        set(&["points", "best_weekly_remaining", "ordinary_held"])
    );
    let slot = &v["slots"][0];
    assert_eq!(
        keys(slot),
        set(&[
            "pid",
            "workstream",
            "class",
            "admitted_at",
            "nested_children",
            "stale"
        ])
    );
    assert_eq!(slot["nested_children"], 3);
    let ws = &v["workstreams"][0];
    assert_eq!(
        keys(ws),
        set(&["name", "admissions", "slots", "waiting", "token_share"])
    );
    assert_eq!(ws["token_share"], Value::Null);
}

#[test]
fn weekly_only_profile_has_no_session() {
    let r = Root::new(&[("alpha", 62.0, 71.0), ("beta", -1.0, 38.0)]);
    let v = json(Some(&r.root), &Settings::default());
    let beta = v["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "beta")
        .unwrap();
    assert_eq!(beta["session"], Value::Null);
    assert_eq!(beta["session_window"], false);
    assert_eq!(beta["invalid_reading"], false);
    assert_eq!(beta["active"], false);
    assert_eq!(beta["eligible"], true);
    assert_eq!(beta["weekly"]["used_pct"], 38.0);
    assert!(
        render_table(&view(Some(&r.root), &Settings::default(), None, NOW))
            .contains("no session window")
    );
}

/// Crusty round 1 on #1831: the governor never says "no session limit", the
/// wording #1818 rejected because the reading cannot tell the cases apart.
#[test]
fn status_never_says_no_session_limit() {
    let r = Root::new(&[("alpha", 62.0, 71.0), ("beta", -1.0, 38.0)]);
    let table = render_table(&view(Some(&r.root), &Settings::default(), None, NOW));
    assert!(!table.contains("no session limit"), "{table}");
}

/// Crusty round 1 on #1831: a session reading out of range makes the
/// profile unknown (out of the pool), and status says why.
#[test]
fn an_invalid_session_reading_is_reported_and_leaves_the_pool() {
    let r = Root::new(&[("alpha", 150.0, 20.0), ("beta", 10.0, 30.0)]);
    let v = json(Some(&r.root), &Settings::default());
    let alpha = v["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "alpha")
        .unwrap();
    assert_eq!(alpha["invalid_reading"], true, "{v}");
    assert_eq!(alpha["session_window"], true);
    assert_eq!(alpha["session"], Value::Null);
    assert_eq!(alpha["weekly"], Value::Null);
    // beta alone is in the pool: 70 weekly points left, not alpha's 80.
    assert_eq!(v["reserve"]["best_weekly_remaining"], 70.0);
    let table = render_table(&view(Some(&r.root), &Settings::default(), None, NOW));
    assert!(table.contains("invalid reading"), "{table}");
}

#[test]
fn reserve_reports_whether_ordinary_work_is_held() {
    let r = Root::new(&[("alpha", 0.0, 95.0)]);
    let v = json(Some(&r.root), &Settings::default());
    assert_eq!(v["reserve"]["points"], 10);
    assert_eq!(v["reserve"]["best_weekly_remaining"], 5.0);
    assert_eq!(v["reserve"]["ordinary_held"], true);
}

#[test]
fn disabled_document_has_null_limit_and_empty_lists() {
    let r = Root::new(&[("alpha", 10.0, 10.0)]);
    seed_state(&r, std::process::id());
    let off = Settings {
        enabled: false,
        ..Settings::default()
    };
    let v = json(Some(&r.root), &off);
    assert_eq!(v["enabled"], false);
    assert_eq!(v["limit"], Value::Null);
    for list in ["profiles", "slots", "queue", "workstreams"] {
        assert_eq!(v[list], serde_json::json!([]), "{list}");
    }
}

#[test]
fn status_changes_nothing_and_marks_dead_slots_stale() {
    let r = Root::new(&[("alpha", 10.0, 10.0)]);
    seed_state(&r, dead_pid());
    let before = r.snapshot();
    let v = json(Some(&r.root), &Settings::default());
    let _ = render_table(&view(Some(&r.root), &Settings::default(), None, NOW));
    assert_eq!(
        r.snapshot(),
        before,
        "status must not write, reclaim or quarantine"
    );
    assert_eq!(v["slots"][0]["stale"], true);
}

#[test]
fn status_creates_no_governor_dir_and_leaves_corrupt_state_in_place() {
    let r = Root::new(&[("alpha", 10.0, 10.0)]);
    let before = r.snapshot();
    let v = json(Some(&r.root), &Settings::default());
    assert_eq!(r.snapshot(), before);
    assert!(!r.gov().exists());
    assert_eq!(v["fail_open_reason"], Value::Null);
    std::fs::create_dir(r.gov()).unwrap();
    std::fs::write(r.gov().join("state.json"), "{bad").unwrap();
    let before = r.snapshot();
    let _ = json(Some(&r.root), &Settings::default());
    assert_eq!(r.snapshot(), before);
}

#[test]
fn status_without_a_root_reports_and_creates_nothing() {
    let v = json(None, &Settings::default());
    assert_eq!(v["enabled"], true);
    assert_eq!(v["profiles"], serde_json::json!([]));
}

#[test]
fn manifest_path_is_never_printed() {
    let r = Root::new(&[("alpha", 10.0, 10.0)]);
    let manifest = r.tmp.path().join("SECRET-MANIFEST-PATH.json");
    std::fs::write(&manifest, "{}").unwrap();
    let sv = view(Some(&r.root), &Settings::default(), Some(&manifest), NOW);
    let text = format!(
        "{}\n{}",
        serde_json::to_string(&sv).unwrap(),
        render_table(&sv)
    );
    assert!(!text.contains("SECRET-MANIFEST-PATH"), "{text}");
}

#[test]
fn display_safe_replaces_controls_and_truncates() {
    assert_eq!(display_safe("ok-name"), "ok-name");
    assert_eq!(display_safe("a\u{1b}[31mb\n"), "a?[31mb?");
    assert_eq!(display_safe("x\u{7f}y\u{9b}"), "x?y?");
    assert_eq!(display_safe(&"z".repeat(100)).chars().count(), 64);
}
