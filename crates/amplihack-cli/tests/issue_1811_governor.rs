//! Issue #1811: the capacity governor, end to end through the real
//! `amplihack` binary. See `issue_1811_fixture` for the isolation rules and
//! docs/reference/capacity-governor.md for the contract.

#![cfg(unix)]

use crate::issue_1811_fixture::{Fixture, describe, now, text};
use std::fs;

const DENIED: i32 = 80;

#[test]
fn no_readings_fails_open_and_status_names_the_reason() {
    let f = Fixture::new();
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 1);
    let st = f.run(&["governor", "status", "--json"]);
    assert!(st.status.success(), "{}", describe(&st));
    let v: serde_json::Value = serde_json::from_slice(&st.stdout).unwrap();
    assert_eq!(v["fail_open_reason"]["code"], "no-readings", "{v}");
}

#[test]
fn weekly_exhaustion_exits_80_without_starting_claude() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 100.0), ("beta", -1.0, 100.0)]);
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(DENIED), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 0, "the child must not start");
    let err = text(&out.stderr);
    assert!(
        err.contains("governor: not started (weekly-exhausted)"),
        "{err}"
    );
    assert!(
        !text(&out.stdout).contains("governor"),
        "admission writes nothing to stdout"
    );
}

#[test]
fn launch_is_governed_too() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 100.0), ("beta", 10.0, 100.0)]);
    let out = f.run(&["launch", "--", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(DENIED), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 0);
}

#[test]
fn room_on_another_profile_admits_and_says_so_on_stderr_only() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 100.0), ("beta", 10.0, 30.0)]);
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.claude_runs(), 1);
    assert!(
        text(&out.stderr).contains("governor: admitted (room)"),
        "{}",
        describe(&out)
    );
    assert!(
        !text(&out.stdout).contains("governor"),
        "{}",
        describe(&out)
    );
}

#[test]
fn room_on_the_active_profile_prints_nothing() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 30.0), ("beta", 10.0, 30.0)]);
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        !text(&out.stderr).contains("governor"),
        "{}",
        describe(&out)
    );
}

#[test]
fn kill_switch_runs_as_before_and_creates_no_state() {
    for off in ["off", "0", "false"] {
        let f = Fixture::new();
        f.readings(&[("alpha", 10.0, 100.0), ("beta", 10.0, 100.0)]);
        let out = f
            .cmd(&["claude", "--", "-p", "hi"])
            .env("AMPLIHACK_GOVERNOR", off)
            .output()
            .unwrap();
        assert!(out.status.success(), "{off}: {}", describe(&out));
        assert_eq!(f.claude_runs(), 1);
        assert!(
            !f.gov().exists(),
            "{off}: no .governor.d with the governor off"
        );
        assert!(
            !text(&out.stderr).contains("governor"),
            "{off}: {}",
            describe(&out)
        );
    }
}

#[test]
fn config_key_turns_the_governor_off() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 100.0)]);
    f.governor_config("enabled = false");
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(!f.gov().exists());
    let on = f
        .cmd(&["claude", "--", "-p", "hi"])
        .env("AMPLIHACK_GOVERNOR", "on")
        .output()
        .unwrap();
    assert_eq!(
        on.status.code(),
        Some(DENIED),
        "the variable outranks the key: {}",
        describe(&on)
    );
}

#[test]
fn concurrent_starts_never_exceed_the_limit() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 20.0)]);
    f.governor_config("max_concurrency = 2");
    let children: Vec<_> = (0..4)
        .map(|i| {
            f.cmd(&[
                "claude",
                "--workstream",
                &format!("ws-{i}"),
                "--",
                "-p",
                "x",
            ])
            .env("AMPLIHACK_GOVERNOR_WAIT", "120")
            .env("STUB_SLEEP", "2")
            .spawn()
            .unwrap()
        })
        .collect();
    for c in children {
        let out = c.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", describe(&out));
    }
    assert_eq!(f.claude_runs(), 4);
    assert!(f.max_overlap() <= 2, "overlap {}", f.max_overlap());
    let st = f.run(&["governor", "status", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&st.stdout).unwrap();
    assert_eq!(
        v["slots"],
        serde_json::json!([]),
        "every slot is released: {v}"
    );
}

#[test]
fn a_dead_process_slot_is_reclaimed() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 20.0)]);
    f.governor_config("max_concurrency = 1");
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    fs::create_dir_all(f.gov()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(f.gov(), fs::Permissions::from_mode(0o700)).unwrap();
    let state = serde_json::json!({
        "version": 1, "limit": 1, "last_evaluated_at": now(), "next_seq": 1,
        "slots": [{"pid": dead, "start_time": 1, "workstream": "ws-dead", "class": "ordinary",
                   "tree_id": "t-dead", "admitted_at": now() - 60, "nested_children": 0}],
        "admissions": [], "fail_open": null
    });
    fs::write(f.gov().join("state.json"), state.to_string()).unwrap();
    fs::set_permissions(
        f.gov().join("state.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let out = f.run(&["claude", "--", "-p", "hi"]);
    assert!(
        out.status.success(),
        "the dead slot must not hold the start: {}",
        describe(&out)
    );
    assert_eq!(f.claude_runs(), 1);
}

#[test]
fn recipe_run_with_limit_one_admits_its_agent_steps_as_nested() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 20.0)]);
    f.governor_config("max_concurrency = 1");
    let recipe = f.recipe_path();
    let out = f
        .cmd(&["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "nested steps must not deadlock: {}",
        describe(&out)
    );
    assert_eq!(f.claude_runs(), 2);
    let tree_ids: std::collections::BTreeSet<String> = fs::read_dir(&f.records)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_str().unwrap().starts_with("env."))
        .flat_map(|p| {
            fs::read_to_string(p)
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|l| l.starts_with("AMPLIHACK_TREE_ID="))
        .collect();
    assert_eq!(
        tree_ids.len(),
        1,
        "every step carries the slot's tree id: {tree_ids:?}"
    );
}

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

#[test]
fn recipe_run_with_exhausted_budget_exits_80_before_the_runner() {
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 100.0), ("beta", 10.0, 100.0)]);
    let recipe = f.recipe_path();
    let out = f
        .cmd(&["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(DENIED), "{}", describe(&out));
    assert!(
        !f.records.join("runner").exists(),
        "recipe-runner-rs must not start"
    );
}

#[test]
fn task_text_never_reaches_state_or_status() {
    const CANARY: &str = "TASKSECRET";
    let f = Fixture::new();
    f.readings(&[("alpha", 10.0, 20.0)]);
    let out = f.run(&["claude", "--", "-p", "TASKSECRET do the thing"]);
    assert!(out.status.success(), "{}", describe(&out));
    let recipe = f.recipe_path();
    let out = f
        .cmd(&["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(&f.work)
        .args(["-c", "task_description=TASKSECRET in a recipe"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let files = f.tree(&f.gov());
    assert!(files.len() > 1, "the governor saved no state");
    for (path, bytes) in files {
        assert!(!text(&bytes).contains(CANARY), "{}", path.display());
    }
    for args in [
        &["governor", "status", "--json"][..],
        &["governor", "status"],
    ] {
        let st = f.run(args);
        assert!(st.status.success(), "{}", describe(&st));
        assert!(!text(&st.stdout).contains(CANARY), "{args:?}");
        assert!(!text(&st.stderr).contains(CANARY), "{args:?}");
    }
}
