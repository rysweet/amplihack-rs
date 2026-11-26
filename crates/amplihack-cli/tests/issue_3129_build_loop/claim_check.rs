//! C3b (#3110, repo-c #480): a claim on the issue is information.
//! `workflow_issue_claim_check.sh` exits 0 for every claim result, keeps its
//! stdout key lines (1.2.213 callers read them) and lists the claims.

#![cfg(target_os = "linux")]

use super::tools::{World, git, text};
use serde_json::{Value, json};

fn claim(number: u32, head: &str) -> Value {
    json!({"number": number, "url": format!("https://github.com/o/r/pull/{number}"),
        "title": "Runner content follow-ups", "state": "OPEN", "mergedAt": null,
        "headRefName": head, "isCrossRepository": false,
        "closingIssuesReferences": [{"number": 463}], "baseRefName": "main",
        "body": "Fixes #463"})
}

#[test]
fn a_claim_by_another_branch_exits_zero_and_keeps_the_claimed_line() {
    let w = World::new();
    w.prs(&json!([claim(480, "feat/runner-follow-ups")]));
    let out = w
        .tool("workflow_issue_claim_check.sh", &w.repo)
        .args(["--issue", "463", "--repo-path"])
        .arg(&w.repo)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("issue_claim_check: claimed issue=463 repo=o/r"),
        "{}",
        text(&out)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("#480"),
        "{}",
        text(&out)
    );
}

#[test]
fn a_claim_on_the_checked_out_branch_exits_zero_and_keeps_the_own_pr_line() {
    let w = World::new();
    git(&w.repo, &["checkout", "-q", "-b", "feat/runner-follow-ups"]);
    w.prs(&json!([claim(480, "feat/runner-follow-ups")]));
    let out = w
        .tool("workflow_issue_claim_check.sh", &w.repo)
        .args(["--issue", "463", "--repo-path"])
        .arg(&w.repo)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("ok-own-pr"),
        "{}",
        text(&out)
    );
}

#[test]
fn json_lists_each_claim_with_its_head_and_whether_it_is_the_runs_own() {
    let w = World::new();
    git(&w.repo, &["checkout", "-q", "-b", "feat/runner-follow-ups"]);
    w.prs(&json!([
        claim(480, "feat/runner-follow-ups"),
        claim(481, "feat/other")
    ]));
    let out = w
        .tool("workflow_issue_claim_check.sh", &w.repo)
        .args(["--json", "--issue", "463", "--repo-path"])
        .arg(&w.repo)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .last()
        .unwrap()
        .to_owned();
    let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {}", text(&out)));
    assert_eq!(v["issue"], "463");
    assert_eq!(v["repo"], "o/r");
    let claims = v["claims"].as_array().unwrap();
    let by = |n: u64| claims.iter().find(|c| c["number"] == n).unwrap().clone();
    assert_eq!(by(480)["own"], true);
    assert_eq!(by(481)["own"], false);
    assert_eq!(by(481)["head"], "feat/other");
}

#[test]
fn json_with_no_claim_reports_an_empty_list() {
    let w = World::new();
    w.prs(&json!([]));
    let out = w
        .tool("workflow_issue_claim_check.sh", &w.repo)
        .args(["--json", "--issue", "463", "--repo-path"])
        .arg(&w.repo)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .last()
        .unwrap()
        .to_owned();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["claims"], json!([]));
}

#[test]
fn a_failed_read_exits_zero() {
    let w = World::new();
    w.flag("list-fails");
    let out = w
        .tool("workflow_issue_claim_check.sh", &w.repo)
        .args(["--issue", "463", "--repo-path"])
        .arg(&w.repo)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("unknown"),
        "{}",
        text(&out)
    );
}

#[test]
fn an_unknown_argument_is_still_a_usage_error() {
    let w = World::new();
    let out = w
        .tool("workflow_issue_claim_check.sh", &w.repo)
        .arg("--no-such-flag")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
}

#[test]
fn a_hostile_head_is_refused_as_a_usage_error_not_passed_on() {
    let w = World::new();
    let out = w
        .tool("workflow_issue_claim_check.sh", &w.repo)
        .args(["--issue", "463", "--head", "--upload-pack=x", "--repo-path"])
        .arg(&w.repo)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
}
