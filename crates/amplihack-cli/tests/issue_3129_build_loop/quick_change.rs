//! C4 (AC8): quick-change keeps the task's own PR title and body.
//! `quick_change_create_pr.sh` runs as quick-change's create-pr step runs it:
//! on a committed change on a `quick/` branch, with the pinned base and the
//! checks summary in the state folder, the runner's values in `RECIPE_VAR_*`
//! and a stand-in gh that keeps the live PR text in files.

#![cfg(target_os = "linux")]

use super::tools::{text, tools_dir};
use crate::support::exec_stub::write_executable;
use crate::support::quick_change_repo::{Repo, git};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

/// `pr create` saves its arguments and body and sets the live text (or a
/// rewritten body when `rewrite-body` exists); `pr view`, `pr edit` and
/// `pr comment` work on the live text and a comments folder. `create-fails`
/// makes `pr create` fail.
const GH: &str = r##"#!/usr/bin/env bash
G="$GH_STATE"
printf '%s\n' "$*" >> "$G/calls.log"
case "${1:-} ${2:-}" in
  "pr create")
    [ -e "$G/create-fails" ] && { echo "HTTP 422: Validation Failed" >&2; exit 1; }
    printf '%s\n' "$@" > "$G/create-args"
    prev=""
    for a in "$@"; do
      case "$a" in --title=*) printf '%s' "${a#--title=}" > "$G/live-title" ;; esac
      [ "$prev" = --body-file ] && cp -- "$a" "$G/create-body"
      prev="$a"
    done
    if [ -e "$G/rewrite-body" ]; then printf 'A generated summary\n' > "$G/live-body"; else cp -- "$G/create-body" "$G/live-body"; fi
    echo "https://github.com/o/r/pull/12" ;;
  "pr view") jq -cn --rawfile t "$G/live-title" --rawfile b "$G/live-body" '{title:$t, body:$b}' ;;
  "pr edit")
    prev=""
    for a in "$@"; do
      case "$a" in --title=*) printf '%s' "${a#--title=}" > "$G/live-title" ;; esac
      [ "$prev" = --body-file ] && cp -- "$a" "$G/live-body"
      prev="$a"
    done ;;
  "pr comment")
    mkdir -p "$G/comments"; n=$(ls "$G/comments" | wc -l); prev=""
    for a in "$@"; do [ "$prev" = --body-file ] && cp -- "$a" "$G/comments/$n.md"; prev="$a"; done ;;
  *) echo "stand-in gh: unexpected call: $*" >&2; exit 1 ;;
esac
"##;

const TASK: &str = "Build PR 2.\n\nPR title: smart-orchestrator and default-workflow run as one goal-driven agent loop\n\nThe description ends with these lines:\nRefs #3129, #3143\nFixes #3110\n";
const SUMMARY: &str = "QUICK_CHANGE_CHECKS=passed ran=1 failed=0 skipped=0";

struct World {
    r: Repo,
    gh: PathBuf,
    bin: PathBuf,
    state: PathBuf,
}

fn world() -> World {
    let r = Repo::new();
    git(&r.wt, &["checkout", "-q", "-b", "quick/issue-3129-test"]);
    r.write("README.md", "# edited\n");
    git(&r.wt, &["add", "-A"]);
    git(&r.wt, &["commit", "-q", "-m", "change"]);
    let wt = fs::canonicalize(&r.wt).unwrap();
    let sum = Command::new("bash")
        .args(["-c", "printf '%s' \"$1\" | cksum | awk '{print $1}'", "_"])
        .arg(&wt)
        .output()
        .unwrap();
    let state = r
        .tmp
        .path()
        .join(".amplihack/state/quick-change")
        .join(String::from_utf8_lossy(&sum.stdout).trim());
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("base"), &r.base).unwrap();
    fs::write(state.join("checks-summary"), format!("{SUMMARY}\n")).unwrap();
    fs::write(state.join("checks-table.md"), "| `cargo test` | exit 0 |\n").unwrap();
    let gh = r.tmp.path().join("ghstate");
    let bin = r.tmp.path().join("bin");
    fs::create_dir_all(&gh).unwrap();
    fs::create_dir_all(&bin).unwrap();
    write_executable(&bin.join("gh"), GH, 0o755);
    World { r, gh, bin, state }
}

fn create(w: &World, task: &str, pr_text: &serde_json::Value) -> Output {
    let wt = fs::canonicalize(&w.r.wt).unwrap();
    let mut c = crate::isolated_amplihack::isolated("bash");
    c.arg(tools_dir().join("quick_change_create_pr.sh"))
        .current_dir(&wt)
        .env("HOME", w.r.tmp.path())
        .env("TMPDIR", w.r.tmp.path())
        .env("PATH", format!("{}:/usr/bin:/bin", w.bin.display()))
        .env("GH_STATE", &w.gh)
        .env(
            "RECIPE_VAR_worktree_setup",
            json!({ "worktree_path": wt }).to_string(),
        )
        .env("RECIPE_VAR_task_description", task)
        .env("RECIPE_VAR_quick_change_base", &w.r.base)
        .env("RECIPE_VAR_quick_change_checks", SUMMARY)
        .env("RECIPE_VAR_quick_change_commit_repaired", "no")
        .env("RECIPE_VAR_quick_change_push_repaired", "no")
        .env("RECIPE_VAR_quick_change_pr_text", pr_text.to_string())
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN");
    c.output().unwrap()
}

fn read(w: &World, name: &str) -> String {
    fs::read_to_string(w.gh.join(name)).unwrap_or_default()
}

fn title_arg(w: &World) -> String {
    read(w, "create-args")
        .lines()
        .find_map(|l| l.strip_prefix("--title=").map(str::to_owned))
        .unwrap_or_default()
}

fn comments(w: &World) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(w.gh.join("comments"))
        .map(|d| {
            d.map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

const OWN_TITLE: &str = "smart-orchestrator and default-workflow run as one goal-driven agent loop";
const OWN_BODY: &str = "One loop builds the change.\n\nRefs #3129, #3143\nFixes #3110\n";

#[test]
fn the_tasks_own_lines_reach_the_pull_request_exactly_and_the_report_goes_in_a_comment() {
    let w = world();
    let out = create(&w, TASK, &json!({"title": OWN_TITLE, "body": OWN_BODY}));
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "https://github.com/o/r/pull/12"
    );
    assert_eq!(title_arg(&w), OWN_TITLE);
    assert_eq!(read(&w, "create-body"), OWN_BODY);
    let c = comments(&w);
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(
        c[0].contains("## Request") && c[0].contains("## Checks"),
        "{c:?}"
    );
    assert!(!w.state.exists(), "the state folder outlived the PR");
}

#[test]
fn a_body_github_rewrote_is_restored_with_one_edit_and_the_run_is_not_failed() {
    let w = world();
    fs::write(w.gh.join("rewrite-body"), "").unwrap();
    let out = create(&w, TASK, &json!({"title": OWN_TITLE, "body": OWN_BODY}));
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let edits = read(&w, "calls.log")
        .lines()
        .filter(|l| l.starts_with("pr edit"))
        .count();
    assert_eq!(edits, 1);
    assert_eq!(read(&w, "live-body"), OWN_BODY);
}

#[test]
fn without_pr_lines_the_title_and_body_come_from_the_task_as_before() {
    let w = world();
    let task = format!("Fix the broken link in the README. {}", "x".repeat(5000));
    let out = create(&w, &task, &json!({"title": "", "body": ""}));
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let title = title_arg(&w);
    assert_eq!(title.chars().count(), 72, "{title}");
    assert!(title.starts_with("Fix the broken link in the README."));
    let body = read(&w, "create-body");
    assert!(body.contains("## Request"), "{body}");
    assert!(
        body.contains(&"x".repeat(3900)),
        "the request keeps 4,000 bytes"
    );
    assert!(!body.contains(&"x".repeat(4100)), "and no more");
    assert!(comments(&w).is_empty());
}

#[test]
fn a_closing_keyword_the_task_never_gave_falls_back_to_the_template() {
    let w = world();
    let out = create(
        &w,
        TASK,
        &json!({"title": OWN_TITLE, "body": "Fixes #999\n"}),
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_ne!(read(&w, "create-body"), "Fixes #999\n");
    assert!(read(&w, "create-body").contains("## Request"));
}

#[test]
fn a_create_that_fails_exits_one_with_no_url_so_the_loop_takes_over() {
    let w = world();
    fs::write(w.gh.join("create-fails"), "").unwrap();
    let out = create(&w, TASK, &json!({"title": OWN_TITLE, "body": OWN_BODY}));
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(out.stdout.is_empty(), "{}", text(&out));
}
