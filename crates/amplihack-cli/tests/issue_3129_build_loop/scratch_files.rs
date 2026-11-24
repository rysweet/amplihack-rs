//! S-D3: a build run makes no file outside its state folder. While a `gh`
//! call runs, the file that holds its error text, not yet redacted, exists,
//! and so does the pull request text read back after `open-pr`. Both are kept
//! in the action's or the reading's scratch folder inside the turn folder, or
//! in the state folder when there is no turn.
//!
//! Each test points TMPDIR at a new, empty folder. A wrapper in front of the
//! stand-in gh writes a line to stderr and lists that folder at every call.
//! Checking TMPDIR after the run would prove nothing, because the files are
//! removed on a normal return wherever they are made.

#![cfg(target_os = "linux")]

use super::tools::{World, git, text};
use crate::support::exec_stub::write_executable;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Logs each call and what the test's TMPDIR holds at that moment, writes a
/// line to stderr, then hands the call to the stand-in gh. The folder comes
/// from PROBE_TMPDIR, since a caller may give gh another TMPDIR. A comment is
/// answered here, since the stand-in has no `pr comment`.
const PROBE: &str = r##"#!/usr/bin/env bash
L="$GH_STATE/probe.log"
printf 'call %s\n' "$*" >> "$L"
find "$PROBE_TMPDIR" -mindepth 1 -maxdepth 1 -printf 'outside %p\n' >> "$L"
echo "stand-in gh: a warning on stderr" >&2
case "${1:-} ${2:-}" in "pr comment") exit 0 ;; esac
exec "$GH_STATE/../bin/gh-stand-in" "$@"
"##;

struct Setup {
    w: World,
    tmp: PathBuf,
    state: PathBuf,
    turn: PathBuf,
    task: PathBuf,
}

fn setup() -> Setup {
    let w = World::new();
    let tmp = w.base.join("tmpdir");
    let state = w.base.join("state");
    let turn = state.join("turns/0001");
    fs::create_dir_all(&tmp).unwrap();
    fs::create_dir_all(&turn).unwrap();
    let task = w.base.join("task.md");
    fs::write(&task, "Build the widget. Fixes #463.\n").unwrap();
    fs::rename(w.bin.join("gh"), w.bin.join("gh-stand-in")).unwrap();
    write_executable(&w.bin.join("gh"), PROBE, 0o755);
    Setup {
        w,
        tmp,
        state,
        turn,
        task,
    }
}

fn cmd(s: &Setup, args: &[&str]) -> Command {
    let mut c = s.w.tool("autodrive_build_goal.sh", &s.w.repo);
    c.args(args)
        .env("TMPDIR", &s.tmp)
        .env("PROBE_TMPDIR", &s.tmp)
        .env("AUTODRIVE_GOAL_CONTRACT", "1")
        .env("AUTODRIVE_STATE_DIR", &s.state)
        .env("AUTODRIVE_TASK_FILE", &s.task)
        .env("RECIPE_VAR_repo_path", &s.w.repo)
        .env(
            "RECIPE_VAR_task_description",
            "Build the widget. Fixes #463.",
        )
        .env("RECIPE_VAR_existing_branch", "feat/x");
    c
}

/// The run's own pull request, #7, open on `feat/x`.
fn own_pr(s: &Setup) {
    let pr = json!({"number": 7, "url": "https://github.com/o/r/pull/7", "state": "OPEN",
        "headRefName": "feat/x", "baseRefName": "main", "isCrossRepository": false,
        "title": "T", "body": "Fixes #463", "isDraft": true});
    s.w.prs(&json!([pr.clone()]));
    s.w.pr_view(&pr);
}

/// Each of `calls` reached gh, so the probe ran on the path under test, and
/// TMPDIR held nothing at any call.
fn assert_nothing_outside(s: &Setup, calls: &[&str]) {
    let log = fs::read_to_string(s.w.gh.join("probe.log")).unwrap_or_default();
    for want in calls {
        assert!(
            log.lines()
                .any(|l| l.starts_with("call ") && l.contains(want)),
            "no gh call with {want:?}:\n{log}"
        );
    }
    assert!(
        !log.lines().any(|l| l.starts_with("outside ")),
        "TMPDIR held files while gh ran:\n{log}"
    );
}

#[test]
fn the_reading_keeps_gh_error_text_inside_the_state_folder() {
    let s = setup();
    s.w.prs(&json!([]));
    let out = cmd(&s, &["read", s.turn.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    // The lookup by branch (build_own_pr), then the claim check on #463.
    assert_nothing_outside(&s, &["pr list --head feat/x", "pr list --repo o/r"]);
}

#[test]
fn open_pr_keeps_gh_error_text_and_the_read_back_inside_the_state_folder() {
    let s = setup();
    git(&s.w.repo, &["checkout", "-q", "-b", "feat/x"]);
    own_pr(&s);
    fs::write(
        s.turn.join("decision.json"),
        json!({"action": "open-pr", "args": {"title": "T", "body": "Fixes #463"}}).to_string(),
    )
    .unwrap();
    let out = cmd(&s, &["act", "open-pr", s.turn.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let r: Value =
        serde_json::from_str(&fs::read_to_string(s.turn.join("result.json")).unwrap()).unwrap();
    assert_eq!(r["outcome"], "already-open", "{r}");
    assert_eq!(r["text"]["result"], "same", "the text was read back: {r}");
    // build_own_pr, build_verify_pr, then pr_text_apply's read-back.
    assert_nothing_outside(
        &s,
        &[
            "pr list --head feat/x",
            "pr view 7 --json number",
            "pr view 7 --json title",
        ],
    );
}

#[test]
fn publish_keeps_gh_error_text_inside_the_state_folder() {
    let s = setup();
    own_pr(&s);
    let body = s.w.base.join("body.md");
    fs::write(&body, "A comment.\n").unwrap();
    let out = cmd(&s, &["publish", body.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    // publish has no turn folder, so the state folder holds the files.
    assert_nothing_outside(
        &s,
        &[
            "pr list --head feat/x",
            "pr view 7 --json number",
            "pr comment 7",
        ],
    );
}
