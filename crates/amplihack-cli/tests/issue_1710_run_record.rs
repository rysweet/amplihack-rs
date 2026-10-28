//! Issue #1710 (D2, D3, R1): `recipe run --detach` leaves a private
//! `<session>.run.json` beside the log, names the launching session as its
//! parent, gives the run `AMPLIHACK_RUN_ID=<session>`, and records the
//! outcome when the run ends. The context file is still removed; the record
//! is not.
//!
//! Black-box through the real binary with the `issue_1606_detach` fixture:
//! cleared environment, temporary HOME, private tmux server. Linux only;
//! skipped when tmux is missing or its socket path would be too long.

#![cfg(target_os = "linux")]

use crate::issue_1606_detach::{Fx, text, tmux_available, wait_for};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

/// Records its environment, then exits at once so the run finishes.
const QUICK_RUNNER: &str = r#"#!/bin/sh
env > "$RECORD_DIR/env.tmp" && mv "$RECORD_DIR/env.tmp" "$RECORD_DIR/env.txt"
exit 0
"#;

const TOKEN_ORIGIN: &str = "https://user:tok3n@github.com/alice/app.git";

fn usable(fx: &Fx) -> bool {
    tmux_available() && fx.tmux_socket_usable()
}

/// Launches a detached run with `extra_env` and returns its session name
/// and the full launch output.
fn launch(fx: &Fx, extra_env: &[(&str, &str)]) -> (String, String) {
    let mut args = fx.recipe_run_args();
    args.extend(
        [
            "--detach",
            "-c",
            "issue_number=1710",
            "-c",
            "pr_number=not-a-number",
            "-c",
            "task_description=SECRET-TASK-TEXT",
        ]
        .map(String::from),
    );
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = fx.run(&args, extra_env);
    let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "{all}");
    let session = text(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("session: ").map(str::to_string))
        .unwrap_or_else(|| panic!("no session line: {all}"));
    (session, all)
}

fn record_path(fx: &Fx, session: &str) -> PathBuf {
    fx.log_dir().join(format!("{session}.run.json"))
}

/// Waits for the record's outcome and returns the parsed record.
fn finished_record(fx: &Fx, session: &str) -> Value {
    let path = record_path(fx, session);
    wait_for(
        Duration::from_secs(60),
        || {
            fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .is_some_and(|v| !v["outcome"].is_null())
        },
        "the run record's outcome",
    );
    serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap()
}

#[test]
fn a_detached_run_leaves_a_private_record_with_its_outcome() {
    let fx = Fx::with_runner(QUICK_RUNNER);
    if !usable(&fx) {
        return;
    }
    let (session, _) = launch(&fx, &[("AMPLIHACK_SESSION_ID", "parent-xyz")]);
    let path = record_path(&fx, &session);
    assert!(path.is_file(), "no record at {}", path.display());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let rec = finished_record(&fx, &session);
    assert_eq!(rec["version"], 1);
    assert_eq!(rec["session"], session.as_str());
    assert_eq!(rec["parent"], "session:parent-xyz");
    assert_eq!(rec["recipe"], "issue-1606-detach");
    assert_eq!(rec["issue_number"], 1710);
    assert!(rec["pr_number"].is_null());
    assert!(rec.get("run_id").is_none());
    assert!(rec["started_at"].as_str().unwrap().ends_with('Z'));
    assert!(rec["cwd"].as_str().unwrap().starts_with('/'));
    let outcome = &rec["outcome"];
    assert!(outcome["exit_code"].is_i64(), "{outcome}");
    let class = outcome["class"].as_str().unwrap();
    assert!(
        [
            "ok",
            "error",
            "usage_limit",
            "transient_transport",
            "environmental",
            "work",
            "indeterminate"
        ]
        .contains(&class),
        "{class}"
    );
    assert!(outcome["finished_at"].as_str().unwrap().ends_with('Z'));

    let raw = fs::read_to_string(&path).unwrap();
    assert!(!raw.contains("SECRET-TASK-TEXT"), "{raw}");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600,
        "the outcome rewrite keeps the mode"
    );
}

#[test]
fn the_context_file_is_removed_but_the_record_survives() {
    let fx = Fx::with_runner(QUICK_RUNNER);
    if !usable(&fx) {
        return;
    }
    let (session, _) = launch(&fx, &[]);
    finished_record(&fx, &session);
    let context = fx.log_dir().join(format!("{session}.context.json"));
    wait_for(
        Duration::from_secs(30),
        || !context.exists(),
        "the context file to be removed",
    );
    assert!(record_path(&fx, &session).is_file());
    let tmp_left: Vec<String> = fs::read_dir(fx.log_dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp."))
        .collect();
    assert!(tmp_left.is_empty(), "{tmp_left:?}");
}

#[test]
fn the_run_gets_its_own_session_as_amplihack_run_id() {
    let fx = Fx::with_runner(QUICK_RUNNER);
    if !usable(&fx) {
        return;
    }
    let (session, _) = launch(&fx, &[("AMPLIHACK_RUN_ID", "outer-run")]);
    wait_for(
        Duration::from_secs(60),
        || fx.records.join("env.txt").is_file(),
        "runner env",
    );
    assert_eq!(fx.runner_env("AMPLIHACK_RUN_ID"), Some(session.clone()));
    // Launched from inside another run, it is that run's child.
    assert_eq!(finished_record(&fx, &session)["parent"], "run:outer-run");
}

#[test]
fn a_parent_known_only_from_claude_code_is_recorded_before_markers_are_removed() {
    let fx = Fx::with_runner(QUICK_RUNNER);
    if !usable(&fx) {
        return;
    }
    let (session, _) = launch(&fx, &[("CLAUDE_CODE_SESSION_ID", "cc-123")]);
    assert_eq!(finished_record(&fx, &session)["parent"], "claude:cc-123");
}

#[test]
fn a_launch_without_any_identity_records_a_null_parent() {
    let fx = Fx::with_runner(QUICK_RUNNER);
    if !usable(&fx) {
        return;
    }
    let (session, _) = launch(&fx, &[]);
    assert!(finished_record(&fx, &session)["parent"].is_null());
}

#[test]
fn a_tokenised_origin_is_reduced_to_owner_and_name_and_never_shown() {
    let fx = Fx::with_runner(QUICK_RUNNER);
    if !usable(&fx) {
        return;
    }
    let project = fx.home.parent().unwrap().join("project");
    for args in [
        vec!["init", "-q"],
        vec!["remote", "add", "origin", TOKEN_ORIGIN],
    ] {
        let ok = amplihack_git::command()
            .args(&args)
            .current_dir(&project)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", &fx.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }
    let (session, launch_output) = launch(&fx, &[]);
    let rec = finished_record(&fx, &session);
    assert_eq!(rec["repo"], "alice/app");
    let raw = fs::read_to_string(record_path(&fx, &session)).unwrap();
    for leak in ["tok3n", "user:", "@github.com"] {
        assert!(!raw.contains(leak), "{leak} in record: {raw}");
        assert!(!launch_output.contains(leak), "{leak} in launch output");
    }
}
