//! Unit tests for `registry.rs`: the version 1 wire format (step 5b section
//! 4), the time helpers (S25), the transitions that keep invariants I1 to I5
//! (C6), the process pair (C5), the conflict queries (U2, U3) and the
//! `registered_at` generation rule (C4). Loading and writing files is in
//! `tests_store.rs`.
//!
//! API under test:
//! - `VERSION: u64 = 1`; `Registry { version: u64, agents: BTreeMap<String,
//!   Entry>, extra: BTreeMap<String, Value> }`; `Registry::new()`
//! - `State { Active, Ended, Stopped }`, `EndedBy { Wrapper, SessionEnd }`
//! - `Entry` (data model 3.1, every field `pub`), `LastRecovery`
//! - `Entry::recorded(&self) -> Option<(u32, u64)>`
//! - `Entry::activate(&mut self, now: i64, launch_id: &str)`,
//!   `end(&mut self, now: i64, by: EndedBy)`, `stop(&mut self, alert: &str)`,
//!   `set_process(&mut self, p: Option<(u32, u64)>)`,
//!   `record_resume(&mut self, now: i64, recovery: LastRecovery)`
//! - `Registry::active_with_conversation(&self, id: &str, except: &str)
//!   -> Option<&str>` and `Registry::session_conflict(&self, socket:
//!   Option<&Path>, session: &str, except: &str, default_socket: &Path)
//!   -> Option<&str>`: the name of the other `active` agent
//! - `fmt_ts(i64) -> String`, `parse_ts(&str) -> Option<i64>`,
//!   `prune_restarts(&[String], now: i64) -> Vec<String>`,
//!   `restarts_within(&[String], now: i64, secs: i64) -> usize`,
//!   `next_registered_at(previous: Option<&str>, now: i64) -> String` (C4)

use super::registry::*;
use serde_json::{Value, json};
use std::path::Path;

const ID: &str = "0b6c3f2e-8d4a-4f1b-9c7e-2a5d6e8f1a3b";
/// 2026-10-30T09:12:04Z.
const T0: i64 = 1_793_351_524;

fn entry_json() -> Value {
    json!({
        "state": "active", "tmux_session": "coord", "tmux_window": "main",
        "tmux_socket": "/tmp/tmux-1000/default", "launch_dir": "/home/alex/src/project",
        "conversation_id": ID, "claude_pid": 41237, "proc_start": 88123456,
        "model": "opus", "add_dirs": [], "skip_permissions": true,
        "amplihack_flags": { "subprocess_safe": false, "no_reflection": true },
        "env": { "CLAUDE_CODE_EFFORT_LEVEL": "high" },
        "registered_at": "2026-10-30T09:12:04Z", "launched_at": "2026-10-30T09:12:04Z",
        "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d", "restarts": []
    })
}

fn entry() -> Entry {
    serde_json::from_value(entry_json()).unwrap()
}

#[test]
fn round_trip_keeps_unknown_fields_at_both_levels_and_drops_them_in_closed_objects() {
    let mut e = entry_json();
    e["future_field"] = json!({"x": [1, 2]});
    e["amplihack_flags"]["future_flag"] = json!(true);
    let doc = json!({ "version": 1, "agents": { "coordinator": e }, "owner": "ops" });
    let reg: Registry = serde_json::from_value(doc).unwrap();
    assert_eq!(reg.version, 1);
    assert_eq!(reg.extra["owner"], json!("ops"));
    let back = serde_json::to_value(&reg).unwrap();
    assert_eq!(back["owner"], json!("ops"));
    let c = &back["agents"]["coordinator"];
    assert_eq!(c["future_field"], json!({"x": [1, 2]}));
    assert!(
        c["amplihack_flags"].get("future_flag").is_none(),
        "closed object"
    );
    let again = serde_json::to_string_pretty(&serde_json::from_value::<Registry>(back).unwrap());
    assert_eq!(again.unwrap(), serde_json::to_string_pretty(&reg).unwrap());
}

#[test]
fn null_and_missing_optionals_read_as_absent_and_are_omitted_on_write() {
    let mut v = entry_json();
    v["lost_at"] = Value::Null;
    v["alert"] = Value::Null;
    for k in [
        "add_dirs",
        "restarts",
        "env",
        "amplihack_flags",
        "skip_permissions",
    ] {
        v.as_object_mut().unwrap().remove(k);
    }
    let e: Entry = serde_json::from_value(v).unwrap();
    assert_eq!((e.lost_at.as_deref(), e.alert.as_deref()), (None, None));
    assert!(e.add_dirs.is_empty() && e.restarts.is_empty() && e.env.is_empty());
    assert!(!e.skip_permissions && e.amplihack_flags.profile.is_none());
    let out = serde_json::to_value(&e).unwrap();
    for k in [
        "lost_at",
        "alert",
        "ended_at",
        "ended_by",
        "last_recovery",
        "effort",
    ] {
        assert!(out.get(k).is_none(), "{k} should be omitted: {out}");
    }
}

#[test]
fn states_are_lowercase_and_ended_by_is_kebab_case() {
    let text = |v: Value| v.as_str().unwrap().to_string();
    let states = [State::Active, State::Ended, State::Stopped].map(|s| json!(s));
    assert_eq!(states.map(text), ["active", "ended", "stopped"]);
    let by = [EndedBy::Wrapper, EndedBy::SessionEnd].map(|b| json!(b));
    assert_eq!(by.map(text), ["wrapper", "session-end"]);
    for bad in ["running", "lost", "Active"] {
        let mut v = entry_json();
        v["state"] = json!(bad);
        assert!(serde_json::from_value::<Entry>(v).is_err(), "{bad}");
    }
}

#[test]
fn a_new_registry_is_version_1_and_empty() {
    let new = json!(Registry::new());
    assert_eq!((VERSION, new), (1, json!({"version": 1, "agents": {}})));
}

#[test]
fn timestamps() {
    assert_eq!(fmt_ts(T0), "2026-10-30T09:12:04Z");
    assert_eq!(parse_ts("2026-10-30T09:12:04Z"), Some(T0));
    let bad = "|2026-10-30 09:12:04|2026-10-30T09:12:04+00:00|2026-10-30T09:12:04\
               |2026-13-01T00:00:00Z|2026-10-30T09:12:04.5Z|1793351524";
    for b in bad.split('|') {
        assert_eq!(parse_ts(b), None, "{b:?}");
    }
}

#[test]
fn restarts_are_pruned_to_24_hours_and_300_seconds_ahead() {
    let r: Vec<String> = [-86_401, -3_600, -86_399, 300, 301, 3_600]
        .iter()
        .map(|d| fmt_ts(T0 + d))
        .chain(["garbage".to_string()])
        .collect();
    let kept = prune_restarts(&r, T0);
    assert_eq!(
        kept,
        [fmt_ts(T0 - 86_399), fmt_ts(T0 - 3_600), fmt_ts(T0 + 300)]
    );
}

#[test]
fn restarts_within_counts_the_window_and_a_little_of_the_future() {
    let r: Vec<String> = [-3_601, -3_600, -10, 0, 300, 301, 3_600]
        .iter()
        .map(|d| fmt_ts(T0 + d))
        .collect();
    assert_eq!(restarts_within(&r, T0, 3_600), 4, "[now-3600, now+300]");
    let shuffled: Vec<String> = r.iter().rev().cloned().collect();
    assert_eq!(
        restarts_within(&shuffled, T0, 3_600),
        4,
        "order does not matter"
    );
    assert_eq!(restarts_within(&[fmt_ts(T0 + 3_600)], T0, 3_600), 0, "S25");
}

/// I1 to I5 of data model section 4.3.
fn assert_invariants(e: &Entry) {
    let ended = e.state == State::Ended;
    assert_eq!(ended, e.ended_at.is_some() && e.ended_by.is_some(), "I1");
    assert_eq!(e.state == State::Stopped, e.alert.is_some(), "I2");
    assert_eq!(e.claude_pid.is_some(), e.proc_start.is_some(), "I3");
    assert!(e.permission_mode.is_none() || !e.skip_permissions, "I4");
    assert!(e.lost_at.is_none() || e.state == State::Active, "I5");
}

#[test]
fn transitions_keep_the_invariants() {
    let mut e = entry();
    e.lost_at = Some(fmt_ts(T0));
    e.stop("stopped: restart limit (3 restarts in the last hour)");
    assert_eq!(e.state, State::Stopped);
    assert_invariants(&e);
    e.activate(T0 + 10, "00000000000000000000000000000001");
    assert_eq!((e.state, e.alert.as_deref()), (State::Active, None));
    assert_eq!(e.launched_at, fmt_ts(T0 + 10));
    assert_eq!(e.launch_id, "00000000000000000000000000000001");
    assert_eq!(e.recorded(), None, "activate clears the process pair");
    assert_invariants(&e);
    e.lost_at = Some(fmt_ts(T0 + 20));
    e.end(T0 + 30, EndedBy::SessionEnd);
    assert_eq!(e.ended_at.as_deref(), Some(fmt_ts(T0 + 30).as_str()));
    assert_eq!(e.ended_by, Some(EndedBy::SessionEnd));
    assert_invariants(&e);
    e.activate(T0 + 40, "00000000000000000000000000000002");
    assert_invariants(&e);
    e.set_process(Some((7, 8)));
    assert_eq!(e.recorded(), Some((7, 8)));
    e.set_process(None);
    assert_invariants(&e);
}

#[test]
fn record_resume_appends_a_restart_and_clears_lost_at() {
    let mut e = entry();
    e.lost_at = Some(fmt_ts(T0 - 60));
    let rec = LastRecovery {
        logged_at: fmt_ts(T0),
        conversation_id: Some(ID.into()),
        lost: Some(fmt_ts(T0 - 60)),
        resumed: Some(fmt_ts(T0)),
        result: "resumed".into(),
    };
    e.record_resume(T0, rec.clone());
    assert_eq!(e.restarts, [fmt_ts(T0)]);
    assert_eq!(e.last_recovery, Some(rec));
    assert_eq!(e.lost_at, None);
    assert_eq!(
        e.launch_id, "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d",
        "A1: not the supervisor's"
    );
    assert_invariants(&e);
}

#[test]
fn the_process_pair_is_read_only_as_a_pair() {
    let mut v = entry_json();
    v.as_object_mut().unwrap().remove("proc_start");
    let e: Entry = serde_json::from_value(v).unwrap();
    assert_eq!(e.recorded(), None, "C5: a lone pid is ignored");
    assert_eq!(entry().recorded(), Some((41237, 88123456)));
}

fn registry(agents: &[(&str, Value)]) -> Registry {
    let map: serde_json::Map<String, Value> = agents
        .iter()
        .map(|(n, e)| (n.to_string(), e.clone()))
        .collect();
    serde_json::from_value(json!({ "version": 1, "agents": map })).unwrap()
}

/// `entry_json()` with the keys of `changes` replaced.
fn with(changes: Value) -> Value {
    let mut v = entry_json();
    for (k, val) in changes.as_object().unwrap() {
        v[k] = val.clone();
    }
    v
}

#[test]
fn one_active_entry_per_conversation() {
    let ended =
        json!({"state": "ended", "ended_at": "2026-10-30T10:00:00Z", "ended_by": "wrapper"});
    let reg = registry(&[("a", entry_json()), ("b", with(ended))]);
    assert_eq!(reg.active_with_conversation(ID, "new"), Some("a"));
    assert_eq!(
        reg.active_with_conversation(ID, "a"),
        None,
        "the entry itself"
    );
    assert_eq!(reg.active_with_conversation("4c1d9e2f", "new"), None);
}

#[test]
fn session_conflicts_compare_sockets_by_component_and_default_an_unset_one() {
    let d = Path::new("/tmp/tmux-1000/default");
    let mut b = with(json!({"tmux_session": "other"}));
    b.as_object_mut().unwrap().remove("tmux_socket");
    let reg = registry(&[
        ("a", with(json!({"tmux_socket": "/tmp/tmux-1000//default"}))),
        ("b", b),
        (
            "c",
            with(json!({"tmux_session": "gone", "state": "stopped", "alert": "x"})),
        ),
    ]);
    let conflict = |sock: Option<&Path>, session: &str, except: &str| {
        reg.session_conflict(sock, session, except, d)
            .map(str::to_string)
    };
    assert_eq!(conflict(Some(d), "coord", "new").as_deref(), Some("a"));
    assert_eq!(conflict(None, "coord", "new").as_deref(), Some("a"));
    assert_eq!(conflict(Some(d), "coord", "a"), None, "the entry itself");
    assert_eq!(conflict(Some(d), "other", "new").as_deref(), Some("b"));
    let elsewhere = Path::new("/run/user/1000/tmux/default");
    assert_eq!(conflict(Some(elsewhere), "coord", "new"), None);
    assert_eq!(conflict(None, "gone", "new"), None, "not active");
}

#[test]
fn registered_at_moves_forward_even_within_one_second() {
    let first = next_registered_at(None, T0);
    assert_eq!(first, fmt_ts(T0));
    assert_eq!(next_registered_at(Some(&first), T0), fmt_ts(T0 + 1), "C4");
    assert_eq!(next_registered_at(Some(&fmt_ts(T0 - 50)), T0), fmt_ts(T0));
    assert_eq!(next_registered_at(Some("garbage"), T0), fmt_ts(T0));
}
