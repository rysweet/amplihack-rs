//! Issue #2333: completion reads the PR head through one REST call, so a
//! refused GraphQL read (`gh pr view`) does not stop a reviewed PR from
//! completing. A read that does not answer, a head that has moved on GitHub
//! and an empty PR number never complete, and never end the run as an
//! invalid decision either: dispatch hands the named fault to the loop-health
//! evaluator, which judges whether another round can get past it.

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

fn evaluator_history(d: &Drive) -> String {
    std::fs::read_to_string(d.fakes.log.join("evaluator-history")).unwrap_or_default()
}

/// The fault reached the evaluator as data, completion was not granted, the
/// dispatch receipt was written, and the evaluator's verdict (STUCK in the
/// fixture) is the loop result rather than an invalid-decision error.
fn assert_evaluated_with(d: &Drive, p: &serde_json::Value, o: &Output, fault: &str) {
    let v = json_output(o);
    assert_eq!(v["source"], "evaluator", "{}", show(o));
    assert_eq!(v["evaluator_invoked"], true, "{}", show(o));
    assert_eq!(v["loop_result"], "STUCK", "{}", show(o));
    assert_eq!(d.evaluators().len(), 1, "{}", show(o));
    let history = evaluator_history(d);
    assert!(
        history.contains("round-1: completion claimed but not granted: ")
            && history.contains(fault),
        "evaluator history must name the fault {fault:?}: {history}"
    );
    let receipt =
        std::path::Path::new(p["private_state"].as_str().unwrap()).with_extension("dispatched");
    assert!(receipt.exists(), "dispatch receipt must be written");
    assert!(!d.marked(), "a refused completion does not mark the phase");
}

#[test]
fn completion_reads_the_head_over_rest_while_graphql_is_refused() {
    let d = Drive::new();
    let p = clean_round(&d);
    let o = dispatch_with(&d, &p, &[("FAKE_GH_FAIL", "pr view")]);
    assert_complete(&d, &o);
    let log = gh_log(&d);
    assert!(log.contains("repos/{owner}/{repo}/pulls/7"), "{log}");
    assert!(!log.contains("pr\u{1f}view"), "{log}");
}

#[test]
fn a_failed_rest_read_goes_to_the_evaluator_as_unavailable() {
    let d = Drive::new();
    let p = clean_round(&d);
    let o = dispatch_with(&d, &p, &[("FAKE_GH_FAIL", "/pulls/7")]);
    assert_evaluated_with(&d, &p, &o, "head unavailable: ");
    assert!(evaluator_history(&d).contains("HTTP 404: Not Found"));
}

#[test]
fn an_evaluator_continue_after_a_failed_read_keeps_the_run_going() {
    let d = Drive::new();
    d.set("eval-verdict", "CONTINUE");
    let p = clean_round(&d);
    let o = dispatch_with(&d, &p, &[("FAKE_GH_FAIL", "/pulls/7")]);
    assert!(o.status.success(), "{}", show(&o));
    let v = json_output(&o);
    assert_eq!(v["loop_result"], "CONTINUE", "{}", show(&o));
    assert_eq!(v["source"], "evaluator", "{}", show(&o));
}

#[test]
fn a_moved_head_names_both_shas_and_never_completes() {
    let d = Drive::new();
    let p = clean_round(&d);
    let moved = "1".repeat(40);
    let o = dispatch_with(&d, &p, &[("FAKE_GH_HEAD", &moved)]);
    let fault = format!(
        "head differs: the PR head on GitHub is {moved}, but the reviewed head is {}",
        d.head()
    );
    assert_evaluated_with(&d, &p, &o, &fault);
}

#[test]
fn an_empty_pr_number_is_resolved_from_the_branch_over_rest() {
    let d = Drive::new();
    let p = clean_round(&d);
    let o = dispatch_with(&d, &p, &[("PR_NUMBER", "")]);
    assert_complete(&d, &o);
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
    assert_evaluated_with(
        &d,
        &p,
        &o,
        "head unavailable: PR_NUMBER is empty and no open PR has head branch quick/demo",
    );
}
