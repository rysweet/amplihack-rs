//! Issue #1811: nested starts and the weekly reserve, end to end through
//! the real `amplihack` binary. Only a start inside a running `recipe run`
//! skips the reserve: under the run's slot, or under a session's slot with
//! the run's recipe marker. A start of its own under an interactive
//! session's slot waits at it. See "Nesting" in docs/reference/capacity-governor.md.

#![cfg(unix)]

use crate::issue_1811_fixture::{Fixture, describe, now, text};
use std::fs;

const DENIED: i32 = 80;

/// Writes a live slot held by this test process, which is an ancestor of
/// every `amplihack` the fixture starts, so `AMPLIHACK_GOVERNOR_PARENT`
/// naming `tree` nests a start under it. `start` is what holds the slot:
/// `agent` for an interactive session, `recipe` for a running recipe.
#[cfg(target_os = "linux")]
fn parent_slot(f: &Fixture, tree: &str, start: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(f.gov()).unwrap();
    fs::set_permissions(f.gov(), fs::Permissions::from_mode(0o700)).unwrap();
    let state = serde_json::json!({
        "version": 1, "limit": 4, "last_evaluated_at": now(), "next_seq": 1,
        "slots": [{"pid": std::process::id(), "start_time": null, "workstream": "ws-session",
                   "class": "ordinary", "tree_id": tree, "admitted_at": now() - 60,
                   "nested_children": 0, "start": start}],
        "admissions": [], "fail_open": null
    });
    let path = f.gov().join("state.json");
    fs::write(&path, state.to_string()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// Crusty round 3 on #1831: an interactive session admitted above the
/// reserve must not keep starting ordinary `recipe run`s once the weekly
/// remainder is below it. The run is nested (it takes no slot and skips the
/// concurrency limit), but it is new work, so it waits at the reserve.
#[cfg(target_os = "linux")]
#[test]
fn recipe_run_under_a_session_below_the_reserve_waits_at_the_reserve() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 95.0), ("beta", 10.0, 95.0)]);
    parent_slot(&f, "t-session", "agent");
    let recipe = f.recipe_path();
    let out = f
        .cmd(&["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .env("AMPLIHACK_GOVERNOR_PARENT", "t-session")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(DENIED), "{}", describe(&out));
    let err = text(&out.stderr);
    assert!(
        err.contains("wait-expired") && err.contains("reserve"),
        "held at the reserve: {}",
        describe(&out)
    );
    assert!(
        !f.records.join("runner").exists(),
        "recipe-runner-rs must not start"
    );
}

/// The other side of the same rule: agent steps (and sub-recipes) under a
/// running recipe's slot finish below the reserve.
#[cfg(target_os = "linux")]
#[test]
fn recipe_run_under_a_running_recipe_below_the_reserve_finishes() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 95.0), ("beta", 10.0, 95.0)]);
    parent_slot(&f, "t-recipe", "recipe");
    let recipe = f.recipe_path();
    let out = f
        .cmd(&["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .env("AMPLIHACK_GOVERNOR_PARENT", "t-recipe")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 2, "both agent steps ran");
}

/// Crusty round 4 on #1831: the usual dev-orchestrator chain. An interactive
/// session holds an `agent` slot; a `recipe run` inside it is admitted above
/// the reserve, nested, with no slot of its own; the weekly remainder then
/// falls below the reserve before its agent steps start. The steps still
/// name only the session's slot as their parent, but the recipe marker the
/// run sets on them says they are inside it, so both run and the recipe
/// finishes.
#[cfg(target_os = "linux")]
#[test]
fn agent_steps_of_a_recipe_nested_under_a_session_finish_below_the_reserve() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 95.0), ("beta", 10.0, 95.0)]);
    let low = f.records.join("low-usage.json");
    fs::copy(f.usage_path(), &low).unwrap();
    f.readings(&[("alpha", 10.0, 50.0), ("beta", 10.0, 50.0)]);
    parent_slot(&f, "t-session", "agent");
    let recipe = f.recipe_path();
    let out = f
        .cmd(&["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .env("AMPLIHACK_GOVERNOR_PARENT", "t-session")
        .env(
            "STUB_BEFORE_STEPS",
            format!("cp '{}' '{}'", low.display(), f.usage_path().display()),
        )
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 2, "both agent steps ran below the reserve");
    let env = fs::read_to_string(f.records.join("runner.env")).unwrap();
    assert!(
        env.contains("AMPLIHACK_GOVERNOR_PARENT=t-session")
            && env.contains("AMPLIHACK_GOVERNOR_RECIPE="),
        "the steps see the session as parent and the run as their recipe: {env}"
    );
    let state = fs::read_to_string(f.gov().join("state.json")).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert_eq!(
        state["slots"].as_array().map(Vec::len),
        Some(1),
        "the nested run and its steps took no slot: {state}"
    );
}

/// The marker is checked against the process tree: one naming a process
/// that is not an ancestor (a leak into a tmux server, say) does not exempt
/// a start under the session from the reserve.
#[cfg(target_os = "linux")]
#[test]
fn a_leaked_recipe_marker_does_not_exempt_a_session_start_from_the_reserve() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 95.0), ("beta", 10.0, 95.0)]);
    parent_slot(&f, "t-session", "agent");
    let mut other = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let out = f
        .cmd(&["claude", "--subprocess-safe", "--", "-p", "hello"])
        .env("AMPLIHACK_GOVERNOR_PARENT", "t-session")
        .env("AMPLIHACK_GOVERNOR_RECIPE", other.id().to_string())
        .output()
        .unwrap();
    let _ = other.kill();
    let _ = other.wait();
    assert_eq!(out.status.code(), Some(DENIED), "{}", describe(&out));
    assert!(text(&out.stderr).contains("reserve"), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 0);
}
