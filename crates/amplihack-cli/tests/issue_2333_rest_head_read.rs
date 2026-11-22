//! Issue #2333: completion reads the PR head through one REST call, so a
//! refused GraphQL read (`gh pr view`) does not stop a reviewed PR from
//! completing. The read is a fact read, not a gate: a valid completion claim
//! is `DONE` whatever it says, with no evaluator call, and the result carries
//! the read as `pr_head` for the agents that follow. A head that moved on
//! GitHub, a read that does not answer and an empty PR number are named
//! there, never turned into a refusal or a demoted verdict in code.

use super::fixture::{Drive, assert_complete, json_output};
use crate::support::amplihack_bin::amplihack_bin;
use crate::support::auto_drive_light::show;
use std::process::Output;

fn clean_round(d: &Drive) -> serde_json::Value {
    d.set("review.json", r#"{"crusty_verdict":"CLEAN","concerns":[]}"#);
    d.record(1);
    let p = d.prepare(1, "");
    d.decide(&p, "complete");
    p
}

fn dispatch_with(d: &Drive, p: &serde_json::Value, env: &[(&str, &str)]) -> Output {
    let mut c = d.cmd(amplihack_bin());
    c.args(["orch", "helper", "autodrive-completion-dispatch"])
        .arg("--private-state")
        .arg(p["private_state"].as_str().unwrap())
        .args(["--child-exit-code", "0"]);
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn gh_log(d: &Drive) -> String {
    std::fs::read_to_string(d.fakes.log.join("gh.log")).unwrap_or_default()
}

/// Completion was granted, and `pr_head` reports the read: its outcome, both
/// heads, and a detail that contains `detail`. The same detail is on stderr.
fn assert_completed_with(d: &Drive, o: &Output, read: &str, github: &str, detail: &str) {
    assert_complete(d, o);
    let v = json_output(o);
    let pr_head = &v["pr_head"];
    assert_eq!(pr_head["read"], read, "{}", show(o));
    assert_eq!(pr_head["github_head"], github, "{}", show(o));
    assert_eq!(pr_head["reviewed_head"], d.head().trim(), "{}", show(o));
    let got = pr_head["detail"].as_str().unwrap_or_default();
    assert!(
        got.contains(detail),
        "pr_head.detail {got:?} lacks {detail:?}"
    );
    if !detail.is_empty() {
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(detail),
            "{}",
            show(o)
        );
    }
}

#[test]
fn completion_reads_the_head_over_rest_while_graphql_is_refused() {
    let d = Drive::new();
    let p = clean_round(&d);
    let o = dispatch_with(&d, &p, &[("FAKE_GH_FAIL", "pr view")]);
    assert_completed_with(&d, &o, "matches", d.head().trim(), "");
    let log = gh_log(&d);
    assert!(log.contains("repos/{owner}/{repo}/pulls/7"), "{log}");
    assert!(!log.contains("pr\u{1f}view"), "{log}");
}

#[test]
fn a_failed_rest_read_is_reported_and_does_not_block_completion() {
    let d = Drive::new();
    let p = clean_round(&d);
    let o = dispatch_with(&d, &p, &[("FAKE_GH_FAIL", "/pulls/7")]);
    assert_completed_with(&d, &o, "unavailable", "", "head unavailable: ");
    let v = json_output(&o);
    assert!(
        v["pr_head"]["detail"]
            .as_str()
            .unwrap()
            .contains("HTTP 404: Not Found"),
        "gh's own error text reaches the next agent: {}",
        show(&o)
    );
}

#[test]
fn a_moved_head_is_reported_with_both_shas_for_the_next_agent() {
    let d = Drive::new();
    let p = clean_round(&d);
    let moved = "1".repeat(40);
    let o = dispatch_with(&d, &p, &[("FAKE_GH_HEAD", &moved)]);
    let detail = format!(
        "head differs: the PR head on GitHub is {moved}, but the reviewed head is {}",
        d.head().trim()
    );
    assert_completed_with(&d, &o, "differs", &moved, &detail);
}

#[test]
fn an_empty_pr_number_is_resolved_from_the_branch_over_rest() {
    let d = Drive::new();
    let p = clean_round(&d);
    let o = dispatch_with(&d, &p, &[("PR_NUMBER", "")]);
    assert_completed_with(&d, &o, "matches", d.head().trim(), "");
    let log = gh_log(&d);
    assert!(
        log.contains("repos/{owner}/{repo}/pulls?state=open&head={owner}:quick/demo"),
        "{log}"
    );
    assert!(log.contains("repos/{owner}/{repo}/pulls/7"), "{log}");
    assert!(!log.contains("pr\u{1f}view"), "{log}");
}

#[test]
fn an_empty_pr_number_with_no_open_pr_is_a_named_blocker() {
    let d = Drive::new();
    let p = clean_round(&d);
    let o = dispatch_with(&d, &p, &[("PR_NUMBER", ""), ("FAKE_GH_PULLS", "[]")]);
    assert_completed_with(
        &d,
        &o,
        "unavailable",
        "",
        "head unavailable: PR_NUMBER is empty and no open PR has head branch quick/demo",
    );
}

/// The agent that receives the result is told what `pr_head` means, so the
/// judgement on a moved or unreadable head is made there and not in code.
#[test]
fn the_recording_step_is_briefed_on_pr_head() {
    let recipe = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../amplifier-bundle/recipes/autodrive-crusty-loop.yaml"),
    )
    .unwrap();
    let step = recipe
        .split("- id: \"step-03-record-crusty-phase\"")
        .nth(1)
        .and_then(|s| s.split("- id: ").next())
        .expect("step-03-record-crusty-phase exists");
    for needle in [
        "`pr_head`",
        "`differs`",
        "`unavailable`",
        "does not undo the DONE",
        "crusty-review-required:<github head>",
        "--match-head-commit",
    ] {
        assert!(step.contains(needle), "step-03 must mention {needle}");
    }
}
