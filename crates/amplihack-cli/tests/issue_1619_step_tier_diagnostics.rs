//! Issue #1619: what `[step_tiers]` could not act on reaches the operator.
//!
//! The per-step override is read in the `amplihack recipe run` process, which
//! performs no launch of its own. Its diagnostics were gathered and dropped
//! there, so a tier file that would not parse, a value that is not a tier and
//! a step id no recipe declares all passed in silence — and a typo'd step id
//! is the likeliest operator error in the feature and the one with no other
//! symptom, because the run simply proceeds on the tiers the recipes declare.
//!
//! Runs the real `amplihack` binary with stdout and stderr in one file, as
//! `issue_1437_final_marker.rs` does, against a stub recipe runner. Every
//! path the run reads or writes is under one temp directory.

#![cfg(unix)]

use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;

/// `amplihack recipe run` probes the runner with `--capabilities` first.
const CAPABILITIES: &str = r#"case "$*" in --capabilities) printf '%b\n' '\173"schema_version":1,"version":"fixture","capabilities":["codex_exec","durable_recovery_v1"]\175'; exit 0 ;; esac
"#;

/// A runner that reports one successful step and does nothing else. The
/// override is applied before the runner starts, so what it does is beside
/// the point here.
const STUB_RUNNER: &str = r#"cat <<'JSON'
{"recipe_name":"issue-1619-step-tiers","success":true,"step_results":[{"step_id":"step-08-implement","status":"success","output":"done"}],"context":{}}
JSON
exit 0
"#;

const RECIPE: &str = r#"name: issue-1619-step-tiers
description: One agent step that declares a tier
version: 1.0.0
context: {}
steps:
  - id: step-08-implement
    type: bash
    command: "true"
"#;

/// Run `amplihack recipe run` with `tier_file` as the instance's
/// `model-tiers.toml`, and return everything it printed.
fn run_with(tier_file: &str) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (home, tools) = (root.join("home"), root.join("tools"));
    let amplihack_home = home.join(".amplihack");
    for dir in [&home, &tools, &amplihack_home] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(amplihack_home.join("model-tiers.toml"), tier_file).unwrap();

    let runner: PathBuf = tools.join("recipe-runner-rs");
    crate::support::exec_stub::write_executable(
        &runner,
        format!("#!/bin/sh\n{CAPABILITIES}{STUB_RUNNER}"),
        0o755,
    );
    let recipe = root.join("recipe.yaml");
    fs::write(&recipe, RECIPE).unwrap();
    let log_path = root.join("round.log");
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
        .env("AMPLIHACK_HOME", &amplihack_home)
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
    assert!(
        status.success(),
        "a tier preference must never stop a run; exit {:?}:\n{text}",
        status.code()
    );
    text
}

#[test]
fn a_step_tiers_value_that_is_not_a_tier_is_reported() {
    let text = run_with("version = 1\n\n[step_tiers]\nstep-08-implement = \"fastt\"\n");
    assert!(
        text.contains("step_tiers.step-08-implement") && text.contains("is not a tier"),
        "the operator is told which entry was dropped and why:\n{text}"
    );
}

#[test]
fn a_step_id_no_recipe_declares_is_reported() {
    let text = run_with("version = 1\n\n[step_tiers]\nstep-99-polish = \"fast\"\n");
    assert!(
        text.contains("step_tiers.step-99-polish") && text.contains("no recipe in this run"),
        "a typo'd step id has no other symptom, so it must have this one:\n{text}"
    );
}

#[test]
fn a_step_id_that_is_not_an_identifier_is_reported() {
    let text = run_with("version = 1\n\n[step_tiers]\n\"../../escape\" = \"fast\"\n");
    assert!(
        text.contains("a step id is an identifier"),
        "a rejected id is named back rather than dropped in silence:\n{text}"
    );
}

#[test]
fn a_tier_file_that_will_not_parse_is_reported() {
    let text = run_with("this is not toml = = =\n");
    assert!(
        text.contains("model-tiers.toml") && text.contains("not valid TOML"),
        "the file the operator wrote is named, home-relative:\n{text}"
    );
    assert!(
        text.contains("~/.amplihack/model-tiers.toml"),
        "run logs get pasted into issues; the absolute path carries the operator's \
         username:\n{text}"
    );
}

#[test]
fn a_step_id_a_recipe_declares_is_applied_without_a_word() {
    let text = run_with("version = 1\n\n[step_tiers]\nstep-08-implement = \"fast\"\n");
    assert!(
        !text.contains("step_tiers.step-08-implement"),
        "an override that matched has nothing to report:\n{text}"
    );
}

#[test]
fn a_run_with_no_step_tier_override_says_nothing_at_all() {
    let text = run_with("version = 1\n\n[claude.fast]\nmodel = \"claude-haiku-5-5\"\n");
    assert!(
        !text.contains("model tiers:"),
        "with nothing to report the run behaves as it did before this feature:\n{text}"
    );
}
