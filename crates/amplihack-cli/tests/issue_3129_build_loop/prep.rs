//! C1c and C1d (#3110, repo-c #480): workflow-prep, run whole through
//! the real binary and runner with PR 1's harness. With no `issue_number`,
//! step-03 takes the issue from the task's `Fixes #463` line, step-03b writes
//! 463 to `issue_number`, and step-03c reports an open pull request on that
//! issue as information: the run goes on.
//!
//! A wrapper in front of the harness's gh answers what workflow-prep adds:
//! the issue read (`gh api repos/{owner}/{repo}/issues/N`, `gh issue view`)
//! and the claim lookup (`gh pr list --repo o/r`). Any other call goes to the
//! harness's gh, which logs it and refuses what it does not know; nothing
//! reaches a real gh. Assertions are on the runner's step results and the
//! values its steps wrote, never on wording.

#![cfg(target_os = "linux")]

use crate::issue_3129_goal_loop::support::{Harness, describe, git};
use crate::support::exec_stub::write_executable;
use serde_json::{Value, json};
use std::fs;

const WRAPPER: &str = r##"#!/usr/bin/env bash
G="$GH_STATE"
case "${1:-} ${2:-}" in
  "api graphql")
    # step-00a's identity pre-flight: the login and its permission on o/r.
    printf '%s\n' "$*" >> "$G/calls.log"
    echo '{"data":{"viewer":{"login":"alice"},"repository":{"nameWithOwner":"o/r","viewerPermission":"WRITE"}}}' ;;
  "api repos/{owner}/{repo}/issues/463"|"api repos/o/r/issues/463")
    printf '%s\n' "$*" >> "$G/calls.log"
    shift 2
    f=""; while [ $# -gt 0 ]; do case "$1" in --jq|-q) f="$2"; shift 2 ;; *) shift ;; esac; done
    echo '{"state":"open","html_url":"https://github.com/o/r/issues/463"}' | if [ -n "$f" ]; then jq -r "$f"; else cat; fi ;;
  "issue view")
    printf '%s\n' "$*" >> "$G/calls.log"
    f=""; while [ $# -gt 0 ]; do case "$1" in --jq|-q) f="$2"; shift 2 ;; *) shift ;; esac; done
    echo '{"state":"OPEN","url":"https://github.com/o/r/issues/463","number":463}' | if [ -n "$f" ]; then jq -r "$f"; else cat; fi ;;
  "pr list")
    printf '%s\n' "$*" >> "$G/calls.log"
    case " $* " in
      *" --state open "*) cat "$G/claims.json" ;;
      *) echo '[]' ;;
    esac ;;
  *) exec "$G/../bin/gh-harness" "$@" ;;
esac
"##;

/// A pull request on another branch that closes #463: the repo-c #480
/// shape, where a run was told to continue that very pull request.
fn claim() -> Value {
    json!([{"number": 480, "url": "https://github.com/o/r/pull/480",
        "title": "Runner content follow-ups", "state": "OPEN", "mergedAt": null,
        "headRefName": "feat/issues-463-464-runner-content-follow-ups", "isCrossRepository": false,
        "closingIssuesReferences": [{"number": 463}], "baseRefName": "main",
        "body": "Fixes #463"}])
}

fn prep(h: &Harness, task: &str) -> (std::process::Output, Value) {
    // origin names the repository by its GitHub URL, as the claim check reads
    // it with `git remote get-url`. workflow-prep only fetches (`git fetch
    // --all`), and skipFetchAll keeps that fetch off the network.
    git(
        &h.repo,
        &["config", "remote.origin.url", "https://github.com/o/r.git"],
    );
    git(&h.repo, &["config", "remote.origin.skipFetchAll", "true"]);
    fs::rename(h.bin.join("gh"), h.bin.join("gh-harness")).unwrap();
    write_executable(&h.bin.join("gh"), WRAPPER, 0o755);
    fs::write(h.gh.join("claims.json"), claim().to_string()).unwrap();
    let task = format!("task_description={task}");
    let repo = format!("repo_path={}", h.repo.display());
    let out = h
        .command(&[
            "recipe",
            "run",
            "workflow-prep",
            "-c",
            &task,
            "-c",
            &repo,
            "--format",
            "json",
            "-w",
            h.repo.to_str().unwrap(),
            "--no-detach",
        ])
        .output()
        .expect("amplihack recipe run");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out, v)
}

fn step<'a>(result: &'a Value, id: &str) -> &'a Value {
    result["step_results"]
        .as_array()
        .unwrap_or_else(|| panic!("no step_results: {result}"))
        .iter()
        .find(|s| s["step_id"] == id)
        .unwrap_or_else(|| panic!("no step {id}: {result}"))
}

fn output_of(result: &Value, id: &str) -> String {
    step(result, id)["output"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[test]
fn a_fixes_line_with_no_issue_number_gives_the_issue_and_its_claims() {
    let h = Harness::new();
    let (out, r) = prep(&h, "Continue the runner content work. Fixes #463.");
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        output_of(&r, "step-03b-extract-issue-number"),
        "463",
        "{}",
        describe(&out)
    );
    // The claim check ran against #463, found #480 and did not end the run.
    assert_eq!(
        step(&r, "step-03c-issue-claim-check")["status"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase(),
        "completed",
        "{r}"
    );
    let claim_line = output_of(&r, "step-03c-issue-claim-check");
    assert!(
        claim_line.starts_with("issue_claim_check: claimed issue=463"),
        "{claim_line}"
    );
    assert!(
        h.calls()
            .iter()
            .any(|c| c.starts_with("pr list") && c.contains("--state open")),
        "{:?}",
        h.calls()
    );
}
