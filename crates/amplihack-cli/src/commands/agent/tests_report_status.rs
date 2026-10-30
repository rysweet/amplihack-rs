//! Unit tests for the `agent status` output in `report.rs`: the text form
//! (R7.4, step 5b section 8.2) and the JSON form (R7.5, B7), including
//! `restarts_last_hour` with the S25 future window.
//!
//! API under test:
//! - `report::status_text(reg: &Registry, now: i64) -> String`
//! - `report::status_json(reg: &Registry, now: i64) -> serde_json::Value`
//! - `registry::Registry` (deserialised from its JSON form)

use super::registry::Registry;
use super::report::*;
use serde_json::{Value, json};

const A: &str = "0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c";
const C: &str = "4c1d9e2f-0a1b-4c2d-8e3f-5a6b7c8d9e0f";
/// 2026-10-30T12:00:00Z.
const NOW: i64 = 1_793_361_600;

fn ts(t: i64) -> String {
    timestamp(t)
}

fn base(dir: &str, session: &str) -> Value {
    json!({
        "state": "active", "tmux_session": session, "tmux_window": "main", "launch_dir": dir,
        "registered_at": "2026-10-30T08:00:00Z", "launched_at": "2026-10-30T08:00:00Z",
        "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d"
    })
}

fn registry() -> Registry {
    let mut coord = base("/srv/project", "coord");
    coord["conversation_id"] = json!(A);
    coord["restarts"] = json!([
        ts(NOW - 3601),
        ts(NOW - 3599),
        ts(NOW - 100),
        ts(NOW + 200),
        ts(NOW + 400)
    ]);
    coord["last_recovery"] = json!({
        "logged_at": "2026-10-30T10:02:11Z", "conversation_id": A,
        "lost": "2026-10-30T10:01:09Z", "resumed": "2026-10-30T10:02:11Z", "result": "resumed"
    });
    let mut ci = base("/srv/ci", "ci");
    ci["state"] = json!("stopped");
    ci["conversation_id"] = json!(C);
    ci["alert"] = json!("stopped: restart limit (3 restarts in the last hour)");
    ci["last_recovery"] = json!({
        "logged_at": "2026-10-30T11:40:00Z", "conversation_id": C,
        "lost": "2026-10-30T11:39:30Z", "result": "stopped: restart limit"
    });
    let mut old = base("/srv/old", "old");
    old["state"] = json!("ended");
    old["ended_at"] = json!("2026-10-30T08:00:00Z");
    old["ended_by"] = json!("wrapper");
    let reg =
        json!({ "version": 1, "agents": { "coordinator": coord, "area-ci": ci, "area-old": old } });
    serde_json::from_value(reg).unwrap()
}

#[test]
fn an_empty_registry() {
    let reg: Registry = serde_json::from_value(json!({ "version": 1, "agents": {} })).unwrap();
    assert_eq!(status_text(&reg, NOW).trim_end(), "no agents registered");
    assert_eq!(status_json(&reg, NOW), json!({ "agents": [] }));
}

#[test]
fn the_text_form() {
    let row = |n: &str, s: &str, c: &str, d: &str, l: &str| {
        format!("{n:<11}  {s:<36}  {c:<36}  {d:<12}  {l}")
            .trim_end()
            .to_string()
    };
    let want = [
        row(
            "NAME",
            "STATE",
            "CONVERSATION",
            "DIRECTORY",
            "LAST RECOVERY",
        ),
        row(
            "area-ci",
            "stopped",
            C,
            "/srv/ci",
            "2026-10-30T11:40:00Z stopped: restart limit",
        ),
        row(
            "area-old",
            "ended 2026-10-30T08:00:00Z (wrapper)",
            "-",
            "/srv/old",
            "-",
        ),
        row(
            "coordinator",
            "active",
            A,
            "/srv/project",
            "2026-10-30T10:02:11Z resumed",
        ),
        "ALERT area-ci: stopped: restart limit (3 restarts in the last hour)".to_string(),
    ];
    let got: Vec<String> = status_text(&registry(), NOW)
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect();
    assert_eq!(got, want);
}

#[test]
fn a_session_end_shows_its_spelling() {
    let mut e = base("/w", "s");
    e["state"] = json!("ended");
    e["ended_at"] = json!("2026-10-30T08:00:00Z");
    e["ended_by"] = json!("session-end");
    let reg: Registry =
        serde_json::from_value(json!({ "version": 1, "agents": { "a": e } })).unwrap();
    let text = status_text(&reg, NOW);
    assert!(
        text.contains("ended 2026-10-30T08:00:00Z (session-end)"),
        "{text}"
    );
}

#[test]
fn text_from_outside_is_sanitised() {
    let mut e = base("/w", "s");
    e["last_recovery"] =
        json!({ "logged_at": "2026-10-30T10:00:00Z", "result": "failed: tmux: a\u{1b}b" });
    let reg: Registry =
        serde_json::from_value(json!({ "version": 1, "agents": { "a": e } })).unwrap();
    let text = status_text(&reg, NOW);
    assert!(text.contains("failed: tmux: a?b"), "{text}");
    assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
}

#[test]
fn the_json_form() {
    let got = status_json(&registry(), NOW);
    let agents = got["agents"].as_array().unwrap();
    let names: Vec<&str> = agents.iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        ["area-ci", "area-old", "coordinator"],
        "sorted by name"
    );
    assert_eq!(
        agents[2],
        json!({
            "name": "coordinator", "state": "active", "conversation_id": A,
            "launch_dir": "/srv/project", "tmux_session": "coord", "tmux_window": "main",
            "restarts_last_hour": 3,
            "last_recovery": {
                "logged_at": "2026-10-30T10:02:11Z", "conversation_id": A,
                "lost": "2026-10-30T10:01:09Z", "resumed": "2026-10-30T10:02:11Z",
                "result": "resumed"
            },
            "alert": null, "ended_at": null, "ended_by": null
        })
    );
    assert_eq!(
        agents[0]["last_recovery"]["resumed"],
        Value::Null,
        "absent is null (B7)"
    );
    assert_eq!(
        agents[0]["alert"],
        json!("stopped: restart limit (3 restarts in the last hour)")
    );
    assert_eq!(agents[1]["ended_by"], json!("wrapper"));
}

#[test]
fn every_key_is_present_for_a_minimal_agent_b7() {
    let reg: Registry =
        serde_json::from_value(json!({ "version": 1, "agents": { "a": base("/w", "s") } }))
            .unwrap();
    let got = status_json(&reg, NOW);
    let a = got["agents"][0].as_object().unwrap();
    let mut keys: Vec<&str> = a.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "alert",
            "conversation_id",
            "ended_at",
            "ended_by",
            "last_recovery",
            "launch_dir",
            "name",
            "restarts_last_hour",
            "state",
            "tmux_session",
            "tmux_window"
        ]
    );
    for k in [
        "alert",
        "conversation_id",
        "ended_at",
        "ended_by",
        "last_recovery",
    ] {
        assert_eq!(a[k], Value::Null, "{k}");
    }
    assert_eq!(a["restarts_last_hour"], json!(0));
}
