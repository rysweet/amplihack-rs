//! Issue #3286: the runner writes its failure-class marker for a failed
//! trusted-bundle check on the real `recipe run` path.
//!
//! The goal loop counts a failed turn as one in which no agent started only
//! when the runner's marker is present and lists no completed and no failed
//! step. A turn with no marker resets the count. So if the runner stopped
//! writing this marker, the loop would never end a run on a repeated
//! bundle-check failure; this test is what notices.
//!
//! Runs the real `amplihack` binary, with stdout and stderr in one file as the
//! goal loop runs a turn, on an `autodrive-*` recipe whose trusted home holds a
//! group-writable bundle. The recipe runner is a stub that records any call
//! other than the capabilities probe, so the test also shows no step ran.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

const PREFIX: &str = "amplihack.recipe.failure_class ";

const RECIPE: &str = r#"name: autodrive-issue-3286-fixture
description: One step that must never run, because the bundle check fails first
version: 1.0.0
context: {}
steps:
  - id: never-runs
    type: bash
    command: "true"
"#;

#[test]
fn a_failed_bundle_check_writes_a_marker_with_no_steps_before_the_error() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (home, tools) = (root.join("home"), root.join("tools"));
    let trusted_home = home.join(".amplihack");
    let bundle = trusted_home.join("amplifier-bundle");
    for dir in [&home, &tools, &bundle] {
        fs::create_dir_all(dir).unwrap();
    }
    // A clone made under umask 0002 leaves its bundle group-writable.
    fs::set_permissions(&bundle, fs::Permissions::from_mode(0o775)).unwrap();

    let calls = root.join("runner-calls");
    let runner = tools.join("recipe-runner-rs");
    let stub = format!(
        r#"#!/bin/sh
case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
printf '%s\n' "$*" >> '{}'
exit 1
"#,
        calls.display()
    );
    crate::support::exec_stub::write_executable(&runner, &stub, 0o755);
    let recipe = root.join("recipe.yaml");
    fs::write(&recipe, RECIPE).unwrap();
    let log_path = root.join("turn.log");
    let log = fs::File::create(&log_path).unwrap();

    let status = crate::isolated_amplihack::isolated(amplihack_bin())
        .args(["recipe", "run"])
        .arg(&recipe)
        .arg("-w")
        .arg(root)
        .current_dir(root)
        .env(
            "PATH",
            format!("{}:/usr/local/bin:/usr/bin:/bin", tools.display()),
        )
        .env("HOME", &home)
        .env("RECIPE_RUNNER_RS_PATH", &runner)
        .env("AMPLIHACK_AGENT_BINARY", "claude")
        .env("AMPLIHACK_HOME", &trusted_home)
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
        .env("AMPLIHACK_SKIP_REFLECTION", "1")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .status()
        .expect("failed to run amplihack");
    let text = fs::read_to_string(&log_path).unwrap();

    assert_eq!(
        status.code(),
        Some(1),
        "a failed bundle check exits 1:\n{text}"
    );
    assert!(
        !calls.exists(),
        "no recipe step may run when the bundle check fails:\n{}",
        fs::read_to_string(&calls).unwrap_or_default()
    );

    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let marker_at = lines
        .iter()
        .rposition(|l| l.starts_with(PREFIX))
        .unwrap_or_else(|| panic!("the runner must write its failure-class marker:\n{text}"));
    let payload: serde_json::Value = serde_json::from_str(&lines[marker_at][PREFIX.len()..])
        .unwrap_or_else(|e| panic!("the marker must be JSON ({e}):\n{text}"));
    assert_eq!(payload["class"], "environmental", "marker: {payload}");
    assert_eq!(payload["retryable"], false, "marker: {payload}");
    assert_eq!(payload["structural_refusal"], false, "marker: {payload}");
    // The goal loop counts a turn as pre-agent only on these two fields being
    // present and empty.
    assert_eq!(
        payload["completed_steps"],
        serde_json::json!([]),
        "marker: {payload}"
    );
    assert_eq!(
        payload["failed_steps"],
        serde_json::json!([]),
        "marker: {payload}"
    );

    let last = lines.last().copied().unwrap_or_default();
    assert!(
        marker_at + 1 < lines.len() && last.starts_with("Error: "),
        "the error line must follow the marker and be the log's last line:\n{text}"
    );
    assert!(
        last.contains(&bundle.display().to_string()),
        "the error names the bundle: {last}"
    );
}
