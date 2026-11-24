//! The build goal through the real shared loop (`autodrive-goal-loop`), with
//! PR 1's harness: the real binary and runner, a bare origin that refuses
//! non-fast-forward pushes, a stub agent whose policy chooses each turn's
//! action from `reading.json`, and a stand-in gh. A wrapper in front of the
//! harness's gh answers what a build adds: `pr list --head`, `pr create`, and
//! the PR text (`pr view --json title,body`, `pr edit`).
//!
//! - A build opens its PR with the task's own title and body byte for byte,
//!   marks it ready, ends `ready`, and never merges (AC9, AC13, condition 1).
//! - A retry whose first attempt already opened the PR continues it:
//!   `open-pr` reports `already-open` and nothing is created (#3110, AC6).

#![cfg(target_os = "linux")]

use crate::issue_3129_goal_loop::support::{Harness, PR_BRANCH, describe};
use crate::support::exec_stub::write_executable;
use serde_json::Value;
use std::fs;
use std::process::Output;

const WRAPPER: &str = r##"#!/usr/bin/env bash
G="$GH_STATE"
case "${1:-} ${2:-}" in
  "pr list")
    printf '%s\n' "$*" >> "$G/calls.log"
    if [ -e "$G/created" ]; then
      jq -cn --arg b "$GH_PR_BRANCH" '[{number:7,url:"https://github.com/o/r/pull/7",state:"OPEN",isDraft:true,headRefName:$b,
        headRefOid:"",baseRefName:"main",isCrossRepository:false,title:"t",body:"b",createdAt:"2026-11-23T00:00:00Z"}]'
    else echo '[]'; fi ;;
  "pr create")
    printf '%s\n' "$*" >> "$G/calls.log"
    prev=""
    for a in "$@"; do
      case "$a" in --title=*) printf '%s' "${a#--title=}" > "$G/live-title" ;; esac
      [ "$prev" = --body-file ] && cp -- "$a" "$G/create-body" && cp -- "$a" "$G/live-body"
      prev="$a"
    done
    : > "$G/created"; echo "https://github.com/o/r/pull/7" ;;
  "pr edit")
    printf '%s\n' "$*" >> "$G/calls.log"
    prev=""
    for a in "$@"; do
      case "$a" in --title=*) printf '%s' "${a#--title=}" > "$G/live-title" ;; esac
      [ "$prev" = --body-file ] && cp -- "$a" "$G/live-body"
      prev="$a"
    done ;;
  "pr view")
    case " $* " in
      *" title,body "*) printf '%s\n' "$*" >> "$G/calls.log"
        jq -cn --rawfile t "$G/live-title" --rawfile b "$G/live-body" '{title:$t, body:$b}' ;;
      *) exec "$G/../bin/gh-harness" "$@" ;;
    esac ;;
  *) exec "$G/../bin/gh-harness" "$@" ;;
esac
"##;

const TITLE: &str = "smart-orchestrator and default-workflow run as one goal-driven agent loop";
const BODY: &str = "One goal-driven loop builds the change.\n\nRefs #3129, #3143\nFixes #3110\n";

/// Turn by turn: open the PR with the task's text, mark it ready, then end.
const POLICY: &str = r#"
# The title and body are files beside the harness log (STUB_LOG).
prev="$(jq -r '.previous.outcome // "none"' "$TURN/reading.json")"
case "$prev" in
  none)
    jq -cn --rawfile t "$STUB_LOG/title.txt" --rawfile b "$STUB_LOG/body.txt" '{action:"open-pr", args:{title:$t, body:$b}, outcomes:[], why:"The branch is pushed and has no pull request, so this turn opens it with the task text."}' ;;
  opened|already-open|text-differs)
    echo '{"action":"mark-ready","args":{},"outcomes":[],"why":"The pull request is open with the task text, so it is marked ready."}' ;;
  *)
    echo '{"action":"ready","args":{"text":"The change is pushed and its pull request is open and ready for review."},"outcomes":[],"why":"The pull request is ready, so the build is done."}' ;;
esac
"#;

fn build(h: &Harness, task: &str) -> Output {
    // The repository is named by its GitHub URL; git reaches the bare origin.
    let origin = h.origin.to_str().unwrap().to_owned();
    crate::issue_3129_goal_loop::support::git(
        &h.repo,
        &["config", "remote.origin.url", "https://github.com/o/r.git"],
    );
    crate::issue_3129_goal_loop::support::git(
        &h.repo,
        &[
            "config",
            &format!("url.{origin}.insteadOf"),
            "https://github.com/o/r.git",
        ],
    );
    fs::rename(h.bin.join("gh"), h.bin.join("gh-harness")).unwrap();
    write_executable(&h.bin.join("gh"), WRAPPER, 0o755);
    fs::write(&h.policy, POLICY).unwrap();
    let state = format!("autodrive_state_dir={}", h.state.display());
    let task = format!("task_description={task}");
    let branch = format!("existing_branch={PR_BRANCH}");
    let repo = format!("repo_path={}", h.repo.display());
    let args = [
        "recipe",
        "run",
        "autodrive-goal-loop",
        "-c",
        "goal_tool=autodrive_build_goal.sh",
        "-c",
        "goal_file=skills/default-workflow/goal-build-pr.md",
        "-c",
        &state,
        "-c",
        &task,
        "-c",
        &branch,
        "-c",
        &repo,
        "-c",
        "no_merge=false",
        "--format",
        "json",
        "-w",
        h.repo.to_str().unwrap(),
        "--no-detach",
    ];
    fs::write(h.base.join("title.txt"), TITLE).unwrap();
    fs::write(h.base.join("body.txt"), BODY).unwrap();
    h.command(&args).output().expect("amplihack recipe run")
}

fn result_of(h: &Harness, turn: &str) -> Value {
    serde_json::from_str(
        &fs::read_to_string(h.state.join("turns").join(turn).join("result.json")).unwrap(),
    )
    .unwrap()
}

#[test]
fn a_build_opens_its_pr_with_the_task_text_marks_it_ready_and_never_merges() {
    let h = Harness::new();
    let task = format!("Build the widget.\n\nPR title: {TITLE}\n\n{BODY}");
    let out = build(&h, &task);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(h.loop_end()["end"], "ready", "{}", describe(&out));
    let calls = h.calls();
    let create: Vec<&String> = calls
        .iter()
        .filter(|c| c.starts_with("pr create"))
        .collect();
    assert_eq!(create.len(), 1, "{calls:?}");
    assert!(create[0].contains(&format!("--title={TITLE}")), "{calls:?}");
    assert_eq!(fs::read_to_string(h.gh.join("create-body")).unwrap(), BODY);
    assert!(
        calls.iter().any(|c| c.starts_with("pr ready 7")),
        "{calls:?}"
    );
    assert!(
        h.merge_calls().is_empty(),
        "a build never merges: {calls:?}"
    );
    assert_eq!(result_of(&h, "001")["outcome"], "opened");
    assert_eq!(result_of(&h, "001")["text"]["result"], "same");
}

#[test]
fn a_retry_continues_the_pr_its_first_attempt_opened() {
    let h = Harness::new();
    fs::write(h.gh.join("created"), "").unwrap();
    fs::write(h.gh.join("live-title"), TITLE).unwrap();
    fs::write(h.gh.join("live-body"), BODY).unwrap();
    let task = format!("Build the widget. Fixes #3110.\n\nPR title: {TITLE}\n\n{BODY}");
    let out = build(&h, &task);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        result_of(&h, "001")["outcome"],
        "already-open",
        "{}",
        describe(&out)
    );
    assert_eq!(result_of(&h, "001")["pr"], 7);
    let calls = h.calls();
    assert!(
        !calls.iter().any(|c| c.starts_with("pr create")),
        "{calls:?}"
    );
    assert_eq!(h.loop_end()["end"], "ready");
}
