//! G1 to G3: the build goal tool, goal contract v1, through its subcommands.
//! Tools are run as the shared loop runs them: `AUTODRIVE_GOAL_CONTRACT=1`,
//! context in `RECIPE_VAR_*`, the task in `AUTODRIVE_TASK_FILE`.

#![cfg(target_os = "linux")]

use super::tools::{World, git, text};
use crate::support::exec_stub::write_executable;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Setup {
    w: World,
    state: PathBuf,
    turn: PathBuf,
    task: PathBuf,
}

fn setup() -> Setup {
    let w = World::new();
    let state = w.base.join("state");
    let turn = state.join("turns/0001");
    fs::create_dir_all(&turn).unwrap();
    let task = w.base.join("task.md");
    fs::write(&task, "Build the widget. Fixes #463.\n").unwrap();
    Setup {
        w,
        state,
        turn,
        task,
    }
}

fn cmd(s: &Setup, args: &[&str]) -> Command {
    let mut c = s.w.tool("autodrive_build_goal.sh", &s.w.repo);
    c.args(args)
        .env("AUTODRIVE_GOAL_CONTRACT", "1")
        .env("AUTODRIVE_STATE_DIR", &s.state)
        .env("AUTODRIVE_TASK_FILE", &s.task)
        .env("RECIPE_VAR_repo_path", &s.w.repo)
        .env(
            "RECIPE_VAR_task_description",
            "Build the widget. Fixes #463.",
        );
    c
}

fn decide(turn: &Path, v: &Value) {
    fs::write(turn.join("decision.json"), v.to_string()).unwrap();
}

fn result(turn: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(turn.join("result.json")).unwrap()).unwrap()
}

#[test]
fn without_the_contract_variable_the_tool_exits_two() {
    let s = setup();
    let out =
        s.w.tool("autodrive_build_goal.sh", &s.w.repo)
            .arg("actions")
            .output()
            .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
}

#[test]
fn actions_lists_the_build_words_and_no_merge() {
    let s = setup();
    let out = cmd(&s, &["actions"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let words: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    for w in [
        "open-pr",
        "mark-ready",
        "review",
        "wait",
        "prepare-worktree",
    ] {
        assert!(words.contains(&w.to_owned()), "{words:?}");
    }
    assert!(
        !words.iter().any(|w| w == "merge"),
        "a build never merges: {words:?}"
    );
    for reserved in ["continue", "wait-owner", "handoff", "ready", "stop"] {
        assert!(
            !words.iter().any(|w| w == reserved),
            "{reserved} is reserved: {words:?}"
        );
    }
}

#[test]
fn final_prints_nothing_before_a_pr_exists() {
    let s = setup();
    s.w.prs(&json!([]));
    let out = cmd(&s, &["final"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        out.stdout.iter().all(u8::is_ascii_whitespace),
        "{}",
        text(&out)
    );
}

#[test]
fn final_prints_nothing_when_the_read_fails() {
    let s = setup();
    s.w.flag("list-fails");
    s.w.flag("view-fails");
    let out = cmd(&s, &["final"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        out.stdout.iter().all(u8::is_ascii_whitespace),
        "{}",
        text(&out)
    );
}

#[test]
fn final_says_merged_for_the_runs_own_merged_pr() {
    let s = setup();
    git(&s.w.repo, &["checkout", "-q", "-b", "feat/x"]);
    s.w.prs(
        &json!([{"number": 7, "state": "MERGED", "headRefName": "feat/x", "baseRefName": "main",
        "isCrossRepository": false, "url": "https://github.com/o/r/pull/7"}]),
    );
    s.w.pr_view(
        &json!({"number": 7, "state": "MERGED", "headRefName": "feat/x", "baseRefName": "main",
        "isCrossRepository": false}),
    );
    let out = cmd(&s, &["final"])
        .env("RECIPE_VAR_existing_branch", "feat/x")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "merged",
        "{}",
        text(&out)
    );
}

#[test]
fn the_reading_names_the_issue_and_how_it_was_found() {
    let s = setup();
    s.w.prs(&json!([]));
    let out = cmd(&s, &["read", s.turn.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let g: Value =
        serde_json::from_str(&fs::read_to_string(s.turn.join("goal.json")).unwrap()).unwrap();
    assert_eq!(g["schema"], 1);
    assert_eq!(g["issue"]["number"], 463);
    assert_eq!(g["issue"]["source"], "marked-reference");
}

#[test]
fn the_build_reading_retries_only_the_declined_issue_claim_read() {
    let s = setup();
    s.w.prs(&json!([]));
    fs::rename(s.w.bin.join("gh"), s.w.bin.join("fixture-gh")).unwrap();
    write_executable(
        &s.w.bin.join("gh"),
        r#"#!/bin/bash
case "$*" in
  'pr list --repo o/r --state merged '*)
    if [ ! -e "$GH_STATE/declined" ]; then
      printf '%s\n' "$*" >> "$GH_STATE/calls.log"
      touch "$GH_STATE/declined"
      echo 'GraphQL: API rate limit already exceeded' >&2
      exit 1
    fi ;;
  'api rate_limit')
    printf '%s\n' "$*" >> "$GH_STATE/calls.log"
    jq -n --argjson reset "$(( $(date +%s) + 60 ))" \
      '{resources:{graphql:{remaining:0, reset:$reset}}}'
    exit $? ;;
esac
exec fixture-gh "$@"
"#,
        0o755,
    );
    write_executable(
        &s.w.bin.join("sleep"),
        "#!/bin/bash\nprintf '%s\\n' \"$1\" >> \"$GH_STATE/sleeps\"\n",
        0o755,
    );

    let out = cmd(&s, &["read", s.turn.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let reading: Value =
        serde_json::from_slice(&fs::read(s.turn.join("goal.json")).unwrap()).unwrap();
    assert_eq!(reading["issue"]["error"], Value::Null);
    let calls = s.w.calls();
    assert_eq!(calls.len(), 5, "{calls:?}");
    assert!(calls[0].starts_with("pr list --repo o/r --state open "));
    assert!(calls[1].starts_with("pr list --repo o/r --state merged "));
    assert_eq!(calls[2], "api rate_limit");
    assert_eq!(calls[3], calls[1]);
    let sleeps = fs::read_to_string(s.w.gh.join("sleeps")).unwrap();
    let delay: u32 = sleeps.trim_end().parse().unwrap();
    assert!((50..=60).contains(&delay), "{delay}");
}

#[test]
fn the_reading_marks_a_claim_on_the_run_branch_and_one_on_another_branch() {
    let s = setup();
    let claim = |n: u32, head: &str| {
        json!({"number": n, "url": format!("https://github.com/o/r/pull/{n}"),
        "state": "OPEN", "mergedAt": null, "headRefName": head, "isCrossRepository": false,
        "closingIssuesReferences": [{"number": 463}], "baseRefName": "main", "body": "Fixes #463", "title": "t"})
    };
    s.w.prs(&json!([claim(480, "feat/x"), claim(481, "feat/other")]));
    let out = cmd(&s, &["read", s.turn.to_str().unwrap()])
        .env("RECIPE_VAR_existing_branch", "feat/x")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let g: Value =
        serde_json::from_str(&fs::read_to_string(s.turn.join("goal.json")).unwrap()).unwrap();
    let claims = g["issue"]["claims"].as_array().unwrap();
    let rel = |n: u64| {
        claims
            .iter()
            .find(|c| c["pr"] == n || c["number"] == n)
            .unwrap()["relation"]
            .clone()
    };
    assert_eq!(rel(480), "run-branch");
    assert_eq!(rel(481), "other-branch");
}

#[test]
fn a_failed_pr_read_is_recorded_as_unreadable_and_the_read_still_succeeds() {
    let s = setup();
    s.w.flag("list-fails");
    let out = cmd(&s, &["read", s.turn.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let g: Value =
        serde_json::from_str(&fs::read_to_string(s.turn.join("goal.json")).unwrap()).unwrap();
    assert_ne!(
        g["prs"]["error"],
        Value::Null,
        "a failed read must carry its reason: {g}"
    );
}

#[test]
fn the_reading_lists_host_inputs_the_worktree_lacks() {
    let s = setup();
    fs::write(s.w.repo.join(".gitignore"), ".venv/\n").unwrap();
    fs::create_dir_all(s.w.repo.join(".venv")).unwrap();
    s.w.prs(&json!([]));
    cmd(&s, &["read", s.turn.to_str().unwrap()])
        .output()
        .unwrap();
    let g: Value =
        serde_json::from_str(&fs::read_to_string(s.turn.join("goal.json")).unwrap()).unwrap();
    assert!(g["host_inputs"].is_object(), "{g}");
    assert_eq!(g["host_inputs"]["launch"], json!([".venv"]));
}

#[test]
fn checks_passed_lists_only_entries_at_the_current_local_head() {
    let s = setup();
    s.w.prs(&json!([]));
    let head = git(&s.w.repo, &["rev-parse", "HEAD"]);
    let old = "0".repeat(40);
    fs::create_dir_all(&s.state).unwrap();
    fs::write(
        s.state.join("checks.json"),
        json!([{"check": "hooks", "head": head, "outcome": "pass"},
               {"check": "tests", "head": old, "outcome": "pass"}])
        .to_string(),
    )
    .unwrap();
    cmd(&s, &["read", s.turn.to_str().unwrap()])
        .output()
        .unwrap();
    let g: Value =
        serde_json::from_str(&fs::read_to_string(s.turn.join("goal.json")).unwrap()).unwrap();
    let list = g["checks_passed"]["entries"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{g}");
    assert_eq!(list[0]["check"], "hooks");
    assert_eq!(g["checks_passed"]["reported_by"], "agent");
}

#[test]
fn open_pr_without_a_decision_file_writes_a_result_and_exits_zero() {
    let s = setup();
    let out = cmd(&s, &["act", "open-pr", s.turn.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(result(&s.turn)["outcome"].is_string());
}

#[test]
fn open_pr_with_an_empty_title_is_invalid_and_creates_nothing() {
    let s = setup();
    decide(
        &s.turn,
        &json!({"action": "open-pr", "args": {"title": "", "body": "b"}}),
    );
    cmd(&s, &["act", "open-pr", s.turn.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(result(&s.turn)["outcome"], "invalid-args");
    assert!(
        !s.w.calls().iter().any(|c| c.starts_with("pr create")),
        "{:?}",
        s.w.calls()
    );
}

#[test]
fn open_pr_refuses_a_secret_in_the_body_and_creates_nothing() {
    let s = setup();
    decide(
        &s.turn,
        &json!({"action": "open-pr", "args": {"title": "T",
        "body": "token ghp_abcdefghijklmnopqrstuvwxyz0123456789"}}),
    );
    cmd(&s, &["act", "open-pr", s.turn.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(result(&s.turn)["outcome"], "refused-secret");
    assert!(!s.w.calls().iter().any(|c| c.starts_with("pr create")));
}

#[test]
fn open_pr_refuses_a_closing_keyword_that_the_task_never_gave() {
    let s = setup();
    decide(
        &s.turn,
        &json!({"action": "open-pr", "args": {"title": "T", "body": "Fixes #999"}}),
    );
    cmd(&s, &["act", "open-pr", s.turn.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(result(&s.turn)["outcome"], "invalid-args");
}

#[test]
fn open_pr_reports_already_open_for_a_retry_of_the_runs_own_pr() {
    let s = setup();
    git(&s.w.repo, &["checkout", "-q", "-b", "feat/x"]);
    let pr = json!({"number": 7, "url": "https://github.com/o/r/pull/7", "state": "OPEN",
        "headRefName": "feat/x", "baseRefName": "main", "isCrossRepository": false,
        "title": "T", "body": "Fixes #463", "isDraft": true});
    s.w.prs(&json!([pr.clone()]));
    s.w.pr_view(&pr);
    decide(
        &s.turn,
        &json!({"action": "open-pr", "args": {"title": "T", "body": "Fixes #463"}}),
    );
    cmd(&s, &["act", "open-pr", s.turn.to_str().unwrap()])
        .env("RECIPE_VAR_existing_branch", "feat/x")
        .output()
        .unwrap();
    let r = result(&s.turn);
    assert_eq!(r["outcome"], "already-open", "{r}");
    assert_eq!(r["pr"], 7);
    assert!(
        !s.w.calls().iter().any(|c| c.starts_with("pr create")),
        "{:?}",
        s.w.calls()
    );
}

#[test]
fn mark_ready_on_the_runs_own_open_pr_marks_it_and_reads_back_the_draft_flag() {
    let s = setup();
    git(&s.w.repo, &["checkout", "-q", "-b", "feat/x"]);
    let pr = json!({"number": 7, "url": "https://github.com/o/r/pull/7", "state": "OPEN",
        "headRefName": "feat/x", "baseRefName": "main", "isCrossRepository": false, "isDraft": false});
    s.w.prs(&json!([pr.clone()]));
    s.w.pr_view(&pr);
    decide(&s.turn, &json!({"action": "mark-ready", "args": {}}));
    cmd(&s, &["act", "mark-ready", s.turn.to_str().unwrap()])
        .env("RECIPE_VAR_existing_branch", "feat/x")
        .output()
        .unwrap();
    assert!(s.w.gh.join("ready-called").exists(), "{:?}", s.w.calls());
    assert!(
        ["marked-ready", "already-ready"].contains(&result(&s.turn)["outcome"].as_str().unwrap())
    );
}

#[test]
fn mark_ready_refuses_a_cross_repository_pr() {
    let s = setup();
    git(&s.w.repo, &["checkout", "-q", "-b", "feat/x"]);
    let pr = json!({"number": 7, "url": "https://github.com/o/r/pull/7", "state": "OPEN",
        "headRefName": "feat/x", "baseRefName": "main", "isCrossRepository": true, "isDraft": true});
    s.w.prs(&json!([pr.clone()]));
    s.w.pr_view(&pr);
    decide(&s.turn, &json!({"action": "mark-ready", "args": {}}));
    cmd(&s, &["act", "mark-ready", s.turn.to_str().unwrap()])
        .env("RECIPE_VAR_existing_branch", "feat/x")
        .output()
        .unwrap();
    assert!(!s.w.gh.join("ready-called").exists());
    let o = result(&s.turn)["outcome"].as_str().unwrap().to_owned();
    assert!(o == "refused-cross-repo" || o == "refused", "{o}");
}

#[test]
fn mark_ready_still_works_when_the_worktree_was_removed() {
    let s = setup();
    let pr = json!({"number": 7, "url": "https://github.com/o/r/pull/7", "state": "OPEN",
        "headRefName": "feat/gone", "baseRefName": "main", "isCrossRepository": false, "isDraft": true});
    s.w.prs(&json!([pr.clone()]));
    s.w.pr_view(&pr);
    decide(&s.turn, &json!({"action": "mark-ready", "args": {}}));
    cmd(&s, &["act", "mark-ready", s.turn.to_str().unwrap()])
        .env("RECIPE_VAR_existing_branch", "feat/gone")
        .env("RECIPE_VAR_pr_number", "7")
        .output()
        .unwrap();
    assert!(s.w.gh.join("ready-called").exists(), "{:?}", s.w.calls());
}

#[test]
fn a_build_never_calls_pr_merge_whatever_no_merge_says() {
    let s = setup();
    for action in ["open-pr", "mark-ready", "review", "wait"] {
        decide(
            &s.turn,
            &json!({"action": action, "args": {"title": "T", "body": "b"}}),
        );
        cmd(&s, &["act", action, s.turn.to_str().unwrap()])
            .env("RECIPE_VAR_no_merge", "false")
            .output()
            .unwrap();
        let _ = fs::remove_file(s.turn.join("result.json"));
    }
    assert!(
        !s.w.calls().iter().any(|c| c.starts_with("pr merge")),
        "{:?}",
        s.w.calls()
    );
}

#[test]
fn publish_exits_one_when_no_pr_exists() {
    let s = setup();
    s.w.prs(&json!([]));
    let body = s.w.base.join("body.md");
    fs::write(&body, "A comment.\n").unwrap();
    let out = cmd(&s, &["publish", body.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        !s.w.calls().iter().any(|c| c.starts_with("issue comment")),
        "never on the issue"
    );
}
