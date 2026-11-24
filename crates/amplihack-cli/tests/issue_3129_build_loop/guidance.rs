//! A1: operator guidance widens the PR-text allow list; guidance an agent step
//! wrote does not. Two layers are tested.
//!
//! - `amplihack recipe guidance show --operator-only`, through the real binary.
//! - `autodrive_build_goal.sh act open-pr`, with a stand-in `amplihack` named
//!   by `AMPLIHACK_BIN` so the tool's call and the way it reads the answer are
//!   observed on their own.
//!
//! Assertions are on exit codes, argv and result fields. None reads recipe,
//! document or prompt text. No sleeps or timers.

#![cfg(target_os = "linux")]

use super::tools::{World, text};
use crate::support::recipe_guide::{GuideFixture, PR_URL, show, stdout};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PR: [&str; 4] = ["--pr", "7", "--pr-repo", "o/r"];

/// Appends one entry. A step's entry is written as an agent step writes it:
/// inside a recipe run (`AMPLIHACK_RECIPE_RUN_ID` set), which `--from-step`
/// allows and which marks the entry as the step's.
fn append(fx: &GuideFixture, text: &str, from_step: bool) {
    let mut args = vec!["recipe", "guide"];
    if from_step {
        args.push("--from-step");
    }
    args.extend_from_slice(&[PR_URL, "-"]);
    let mut cmd = fx.amplihack(&args);
    if from_step {
        cmd.env("AMPLIHACK_RECIPE_RUN_ID", "run-3129-step");
    } else {
        cmd.env_remove("AMPLIHACK_RECIPE_RUN_ID");
    }
    let out = fx.run_with_stdin(cmd, Some(text.as_bytes()));
    assert!(out.status.success(), "{}", show(&out));
}

fn guidance_show(fx: &GuideFixture, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["recipe", "guidance", "show"];
    args.extend_from_slice(&PR);
    args.extend_from_slice(extra);
    fx.run_with_stdin(fx.amplihack(&args), None)
}

#[test]
fn show_operator_only_leaves_out_entries_from_agent_steps() {
    let fx = GuideFixture::new();
    append(&fx, "operator note ZQOPERATOR", false);
    append(&fx, "step note ZQSTEP", true);

    let all = guidance_show(&fx, &[]);
    assert!(all.status.success(), "{}", show(&all));
    assert!(
        stdout(&all).contains("ZQOPERATOR") && stdout(&all).contains("ZQSTEP"),
        "without the flag both are listed: {}",
        show(&all)
    );

    let only = guidance_show(&fx, &["--operator-only"]);
    assert_eq!(only.status.code(), Some(0), "{}", show(&only));
    let plain = stdout(&only);
    assert!(plain.contains("ZQOPERATOR"), "{plain}");
    assert!(!plain.contains("ZQSTEP"), "{plain}");
}

#[test]
fn show_operator_only_json_lists_no_step_entry_and_keeps_schema_one() {
    let fx = GuideFixture::new();
    append(&fx, "operator note", false);
    append(&fx, "step note", true);
    let out = guidance_show(&fx, &["--json", "--operator-only"]);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v: Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(v["schema_version"], 1, "{v}");
    let entries = v["entries"].as_array().unwrap_or_else(|| panic!("{v}"));
    assert_eq!(entries.len(), 1, "{v}");
    assert!(
        entries.iter().all(|e| e["from_step"] == json!(false)),
        "{v}"
    );
}

#[test]
fn show_operator_only_with_only_step_entries_prints_nothing_and_exits_zero() {
    let fx = GuideFixture::new();
    append(&fx, "step note ZQSTEPONLY", true);
    let out = guidance_show(&fx, &["--operator-only"]);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert!(!stdout(&out).contains("ZQSTEPONLY"), "{}", show(&out));
}

#[test]
fn wait_does_not_take_operator_only() {
    let fx = GuideFixture::new();
    append(&fx, "operator note", false);
    let mut args = vec!["recipe", "guidance", "wait"];
    args.extend_from_slice(&PR);
    args.extend_from_slice(&["--since", "", "--operator-only"]);
    let out = fx.run_with_stdin(fx.amplihack(&args), None);
    assert_eq!(out.status.code(), Some(2), "{}", show(&out));
}

// ---- the goal tool's side ------------------------------------------------

struct Setup {
    w: World,
    state: PathBuf,
    turn: PathBuf,
    task: PathBuf,
    argv: PathBuf,
}

const TASK_TEXT: &str = "Build the widget. Fixes #463.";

/// A stand-in `amplihack`. `STUB_GUIDANCE_RC` is its exit code; on 0 it prints
/// `STUB_GUIDANCE_TEXT`. Its argv is logged to `$STUB_ARGV`.
const AMPLIHACK: &str = r##"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$STUB_ARGV"
[ "${STUB_GUIDANCE_RC:-0}" -eq 0 ] || exit "$STUB_GUIDANCE_RC"
printf '%s\n' "${STUB_GUIDANCE_TEXT:-}"
"##;

fn setup(pr_url: &str) -> Setup {
    let w = World::new();
    let state = w.base.join("state");
    let turn = state.join("turns/0001");
    fs::create_dir_all(&turn).unwrap();
    let task = w.base.join("task.md");
    fs::write(&task, format!("{TASK_TEXT}\n")).unwrap();
    fs::write(
        state.join("pr.json"),
        json!({"number": 7, "url": pr_url}).to_string(),
    )
    .unwrap();
    let stub = w.base.join("amplihack-stub");
    crate::support::exec_stub::write_executable(&stub, AMPLIHACK, 0o755);
    let argv = w.base.join("argv.log");
    Setup {
        w,
        state,
        turn,
        task,
        argv,
    }
}

fn open_pr(s: &Setup, body: &str, rc: i32, guidance: &str) -> Value {
    fs::write(
        s.turn.join("decision.json"),
        json!({"action": "open-pr", "args": {"title": "T", "body": body}}).to_string(),
    )
    .unwrap();
    let out =
        s.w.tool("autodrive_build_goal.sh", &s.w.repo)
            .args(["act", "open-pr", s.turn.to_str().unwrap()])
            .env("AUTODRIVE_GOAL_CONTRACT", "1")
            .env("AUTODRIVE_STATE_DIR", &s.state)
            .env("AUTODRIVE_TASK_FILE", &s.task)
            .env("RECIPE_VAR_repo_path", &s.w.repo)
            .env("RECIPE_VAR_task_description", TASK_TEXT)
            .env("AMPLIHACK_BIN", s.w.base.join("amplihack-stub"))
            .env("STUB_ARGV", &s.argv)
            .env("STUB_GUIDANCE_RC", rc.to_string())
            .env("STUB_GUIDANCE_TEXT", guidance)
            .output()
            .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    serde_json::from_str(&fs::read_to_string(s.turn.join("result.json")).unwrap()).unwrap()
}

fn argv(s: &Setup) -> Vec<String> {
    fs::read_to_string(&s.argv)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn tree_holds(dir: &Path, needle: &str) -> bool {
    fs::read_dir(dir).unwrap().any(|e| {
        let p = e.unwrap().path();
        if p.is_dir() {
            tree_holds(&p, needle)
        } else {
            fs::read_to_string(&p).is_ok_and(|t| t.contains(needle))
        }
    })
}

#[test]
fn operator_guidance_allows_a_closing_line_the_task_did_not_give() {
    let s = setup(PR_URL);
    let r = open_pr(&s, "Fixes #12", 0, "Please close issue 12.\nFixes #12");
    assert_ne!(r["outcome"], "invalid-args", "{r}");
    let calls = argv(&s);
    assert!(
        calls
            .iter()
            .any(|l| l.contains("--operator-only") && l.contains("guidance") && l.contains(PR_URL)),
        "{calls:?}"
    );
}

#[test]
fn guidance_from_an_agent_step_does_not_widen_the_allowed_lines() {
    // The tool asks for operator entries only; a read that lists none leaves
    // the list as the task gave it.
    let s = setup(PR_URL);
    let r = open_pr(&s, "Fixes #12", 0, "");
    assert_eq!(r["outcome"], "invalid-args", "{r}");
    let refused = r["refused"].as_str().unwrap_or_else(|| panic!("{r}"));
    assert_eq!(refused, "closing reference 12", "{r}");
    assert!(
        argv(&s).iter().all(|l| l.contains("--operator-only")),
        "{:?}",
        argv(&s)
    );
}

#[test]
fn a_guidance_read_that_fails_does_not_widen_the_allowed_lines() {
    let s = setup(PR_URL);
    let r = open_pr(&s, "Fixes #12", 2, "Fixes #12");
    assert_eq!(r["outcome"], "invalid-args", "{r}");
    assert_eq!(r["guidance_unread"], 2, "{r}");
    assert_eq!(r["refused"], "closing reference 12", "{r}");
}

#[test]
fn a_read_that_succeeds_leaves_guidance_unread_out_of_the_result() {
    let s = setup(PR_URL);
    let r = open_pr(&s, "Fixes #12", 0, "");
    assert!(r.get("guidance_unread").is_none(), "{r}");
}

#[test]
fn a_malformed_pr_url_still_reads_the_run_folder_guidance() {
    let s = setup("not a url; rm -rf /");
    let r = open_pr(&s, "Fixes #12", 0, "Fixes #12");
    assert_ne!(r["outcome"], "invalid-args", "{r}");
    let calls = argv(&s);
    assert!(
        calls.iter().any(|l| l.contains("--operator-only")),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .all(|l| !l.contains("not a url") && !l.contains("rm -rf")),
        "a malformed URL never reaches the command: {calls:?}"
    );
}

#[test]
fn a_url_for_another_host_is_dropped_before_the_call() {
    let s = setup("https://example.com/o/r/pull/7");
    open_pr(&s, "Fixes #12", 0, "");
    assert!(
        argv(&s).iter().all(|l| !l.contains("example.com")),
        "{:?}",
        argv(&s)
    );
}

#[test]
fn guidance_text_never_reaches_a_kept_record() {
    let s = setup(PR_URL);
    let r = open_pr(&s, "Fixes #12", 0, "Fixes #12 ZQGUIDANCETOKEN");
    assert_ne!(r["outcome"], "invalid-args", "{r}");
    assert!(
        !tree_holds(&s.state, "ZQGUIDANCETOKEN"),
        "guidance text appears in a kept file under the state folder"
    );
    assert!(
        !Command::new("ls")
            .arg(&s.state)
            .output()
            .unwrap()
            .stdout
            .is_empty()
    );
}
