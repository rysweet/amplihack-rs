//! Unit tests for `scan::observe` (the classification table of design 4.8,
//! with B5) and `scan::refreshed_id` (D5, with the C3 rule that an id held
//! by another active agent is not taken).
//!
//! API under test:
//! - `scan::observe(live: &Liveness, session: &SessionState) -> Observation`
//! - `enum Liveness { Running(&'static str), NotRunning, Unknown(String) }`
//! - `enum Observation { Healthy, SkipRunning(Option<String>),
//!   SkipSession(Option<String>), Lost }` (`Debug`, `PartialEq`); the
//!   `Option` holds the liveness reason (`SkipRunning`) or the tmux reason
//!   (`SkipSession`, B5) when that state is unknown
//! - `tmux::SessionState { Present, Absent, Unknown(String) }`
//! - `scan::refreshed_id(snap: &Snapshot, dirs: &[PathBuf], recorded: (u32, u64),
//!   current: Option<&str>, reg: &Registry, name: &str) -> Option<String>`:
//!   the `sessionId` of the recorded process's sessions file when the process
//!   is alive with that start time, the id is valid, differs from `current`,
//!   and no other `active` agent has it

use super::registry::Registry;
use super::scan::*;
use super::tests_proc_tree::*;
use super::tmux::SessionState;
use serde_json::json;

const ID: &str = "0b6c3f2e-8d4a-4f1b-9c7e-2a5d6e8f1a3b";
const NEW: &str = "6f3b9d2e-4a7c-4e1b-8c5d-2e9a7f1b4d36";

#[test]
fn the_observation_table() {
    use Observation::*;
    let run = Liveness::Running("x");
    let unk = Liveness::Unknown("liveness reason".into());
    let not = Liveness::NotRunning;
    let (present, absent) = (SessionState::Present, SessionState::Absent);
    let unknown = SessionState::Unknown("tmux reason".into());
    for l in [&run, &unk] {
        assert_eq!(observe(l, &present), Healthy);
        assert_eq!(observe(l, &unknown), Healthy);
    }
    assert_eq!(observe(&run, &absent), SkipRunning(None));
    assert_eq!(
        observe(&unk, &absent),
        SkipRunning(Some("liveness reason".into()))
    );
    assert_eq!(observe(&not, &present), SkipSession(None));
    assert_eq!(
        observe(&not, &unknown),
        SkipSession(Some("tmux reason".into()))
    );
    assert_eq!(observe(&not, &absent), Lost);
}

fn registry(agents: serde_json::Value) -> Registry {
    serde_json::from_value(json!({ "version": 1, "agents": agents })).unwrap()
}

fn entry(state: &str, conv: &str) -> serde_json::Value {
    let mut e = json!({
        "state": state, "tmux_session": "s", "tmux_window": "w", "launch_dir": "/w",
        "conversation_id": conv, "registered_at": "2026-10-30T09:00:00Z",
        "launched_at": "2026-10-30T09:00:00Z", "launch_id": "6f1c0d8e2a4b4f0e9c7d3b5a1e2f4c6d"
    });
    if state == "ended" {
        e["ended_at"] = json!("2026-10-30T10:00:00Z");
        e["ended_by"] = json!("wrapper");
    }
    e
}

#[test]
fn refreshed_id_follows_the_recorded_process_d5() {
    let t = tree();
    add(&t, 5000, 1, "claude", &["claude"], 777, 'S');
    session_file(&t, 5000, NEW, "777");
    let s = snap(&t);
    let dirs = [t.config.clone()];
    let reg = registry(json!({ "me": entry("active", ID) }));
    let got = refreshed_id(&s, &dirs, (5000, 777), Some(ID), &reg, "me");
    assert_eq!(got.as_deref(), Some(NEW));
    assert_eq!(
        refreshed_id(&s, &dirs, (5000, 777), Some(NEW), &reg, "me"),
        None
    );
    assert_eq!(
        refreshed_id(&s, &dirs, (5000, 778), Some(ID), &reg, "me"),
        None
    );
    assert_eq!(
        refreshed_id(&s, &dirs, (5001, 777), Some(ID), &reg, "me"),
        None
    );
}

#[test]
fn refreshed_id_skips_an_id_held_by_another_active_agent_c3() {
    let t = tree();
    add(&t, 5000, 1, "claude", &["claude"], 777, 'S');
    session_file(&t, 5000, NEW, "777");
    let (s, dirs) = (snap(&t), [t.config.clone()]);
    let held = registry(json!({ "me": entry("active", ID), "other": entry("active", NEW) }));
    assert_eq!(
        refreshed_id(&s, &dirs, (5000, 777), Some(ID), &held, "me"),
        None
    );
    let ended = registry(json!({ "me": entry("active", ID), "other": entry("ended", NEW) }));
    let got = refreshed_id(&s, &dirs, (5000, 777), Some(ID), &ended, "me");
    assert_eq!(
        got.as_deref(),
        Some(NEW),
        "only an active agent holds an id"
    );
}

#[test]
fn refreshed_id_ignores_an_invalid_session_id() {
    let t = tree();
    add(&t, 5000, 1, "claude", &["claude"], 777, 'S');
    session_file(&t, 5000, "-p", "777");
    let (s, dirs) = (snap(&t), [t.config.clone()]);
    let reg = registry(json!({ "me": entry("active", ID) }));
    assert_eq!(
        refreshed_id(&s, &dirs, (5000, 777), Some(ID), &reg, "me"),
        None
    );
}
