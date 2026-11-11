//! #2342 TDD contracts. These use the real CLI and production shell consumers;
//! they do not implement a replacement readiness controller in the fixture.
//! The validation CLI is proposed as `orch helper autodrive-evidence validate`
//! (JSON stdin, or `--input <private-file>`). It validates a snapshot, never
//! decides whether repository policy requires synchronisation.
//!
//! Static recipe checks cover agent ownership and dependency wiring. They
//! are deliberately NOT evidence of agent judgement or a live waiting trace.
//! A production agent trace remains required before accepting the feature.

#![cfg(unix)]

use super::{field, read, recipe_yaml, step, steps, tool_path};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

mod validation;
mod workflow;

fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/issue_2342_ci_responses.json")).unwrap()
}

/// Additive snapshot: all existing string fields are kept. `current` is the
/// fresh platform observation against which agent assessment is validated.
fn snapshot() -> Value {
    let f = fixture();
    let identity = &f["identity"];
    let head = &f["heads"]["source"];
    let base = &f["heads"]["base"];
    let attempts = json!([{"run_id":801,"attempt":1}]);
    json!({
        "current": {
            "identity":identity, "head_sha":head, "base_sha":base,
            "policy_revision":"ruleset-42-v1", "ci_attempts":attempts,
            "tree_sha":f["heads"]["tree"], "observed_at":"2026-11-11T10:04:00Z",
            "required_checks":[{"name":"build", "app_id":15368}]
        },
        "merge_sync": {
            "identity":identity, "base":"main", "synced":"false", "conflict":"false",
            "head_sha":head, "git_observed":"true", "remote_head_before":head,
            "remote_head_after":head, "head_repository":"acme/widgets", "head_ref":"feature",
            "policy_status":"KNOWN", "update_required":"false", "decision":"no-update",
            "policy_revision":"ruleset-42-v1",
            "policy_sources":["github-rest:branches/main/protection", "github-rest:rules/branches/main"]
        },
        "ci_evidence": {
            "identity":identity, "head_sha":head, "base_sha":base,
            "ci_status":"GREEN", "ci_total":"1", "ci_pending":"0", "ci_failing":"0",
            "ci_signal":"GREEN:0/1", "observation":f["green"], "ci_attempts":attempts
        },
        "qa_evidence": {
            "identity":identity, "head_sha":head, "qa_status":"PASS",
            "qa_round":"merge-ready-round-1", "gadugi_required":"false", "gadugi_status":"NOT_REQUIRED",
            "qa_commands_run":"1", "qa_commands_failed":"0", "ci_attempts":attempts,
            "tree_sha":f["heads"]["tree"], "clean_before":"true", "clean_after":"true"
        },
        "assessment": {
            "identity":identity, "assessed_head_sha":head, "base_sha":base,
            "policy_revision":"ruleset-42-v1", "ci_attempts":attempts,
            "merge_ready_verdict":"MERGE_READY", "blockers":[], "refresh_required":"false"
        }
    })
}

fn diagnostic(out: &Output) -> String {
    format!(
        "exit={}\nstdout={}\nstderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn helper(raw: &str) -> Output {
    let mut cmd = crate::isolated_amplihack::isolated(env!("CARGO_BIN_EXE_amplihack"));
    cmd.args(["orch", "helper", "autodrive-evidence", "validate"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("start evidence helper");
    // A missing CLI command may close stdin before reading it. Preserve that
    // child's diagnostic rather than replacing it with a BrokenPipe panic.
    let _ = child.stdin.take().unwrap().write_all(raw.as_bytes());
    child.wait_with_output().unwrap()
}

fn valid_snapshot() {
    let out = helper(&snapshot().to_string());
    assert!(
        out.status.success(),
        "valid no-op snapshot was rejected: {}",
        diagnostic(&out)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("helper emits structured JSON");
    assert_eq!(v["valid"], true, "{}", diagnostic(&out));
    assert_eq!(v["refresh_required"], false, "{}", diagnostic(&out));
}

fn private_file(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

/// Shell consumers get an isolated HOME and no inherited credentials/context.
/// Only GitHub/Git *observations* are faked; extract-json/field use the real CLI.
fn shell(script: &str, dir: &Path, env: &[(&str, String)]) -> Output {
    let mut cmd = Command::new("bash");
    cmd.env_clear()
        .args(["-c", script])
        .current_dir(dir)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
        .env("AMPLIHACK_ROTATE_AUTOSTART", "0");
    crate::isolated_amplihack::no_git_prompt(&mut cmd);
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.output().expect("run production shell consumer")
}

fn bin_path(dir: &Path) -> String {
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_amplihack"), dir.join("amplihack")).unwrap();
    format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}
