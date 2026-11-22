//! Issue #2333: completion reads the PR head through one REST call, so a
//! refused GraphQL read (`gh pr view`) does not stop a reviewed PR from
//! completing, and a failed REST read reaches the loop with gh's own message.

use super::fixture::{Drive, assert_complete};
use crate::support::amplihack_bin::amplihack_bin;
use crate::support::auto_drive_light::{show, stderr};
use std::process::Output;

fn clean_round(d: &Drive) -> serde_json::Value {
    d.set("review.json", r#"{"crusty_verdict":"CLEAN","concerns":[]}"#);
    d.record(1);
    let p = d.prepare(1, "");
    d.decide(&p, "complete");
    p
}

fn dispatch_with_gh_failing(d: &Drive, p: &serde_json::Value, fail: &str) -> Output {
    d.cmd(amplihack_bin())
        .args(["orch", "helper", "autodrive-completion-dispatch"])
        .arg("--private-state")
        .arg(p["private_state"].as_str().unwrap())
        .args(["--child-exit-code", "0"])
        .env("FAKE_GH_FAIL", fail)
        .output()
        .unwrap()
}

fn gh_log(d: &Drive) -> String {
    std::fs::read_to_string(d.fakes.log.join("gh.log")).unwrap_or_default()
}

#[test]
fn completion_reads_the_head_over_rest_while_graphql_is_refused() {
    let d = Drive::new();
    let p = clean_round(&d);
    let o = dispatch_with_gh_failing(&d, &p, "pr view");
    assert_complete(&d, &o);
    let log = gh_log(&d);
    assert!(log.contains("repos/{owner}/{repo}/pulls/7"), "{log}");
    assert!(!log.contains("pr\u{1f}view"), "{log}");
}

#[test]
fn a_failed_rest_read_carries_the_gh_message() {
    let d = Drive::new();
    let p = clean_round(&d);
    let o = dispatch_with_gh_failing(&d, &p, "/pulls/7");
    assert!(!o.status.success(), "{}", show(&o));
    assert!(stderr(&o).contains("HTTP 404: Not Found"), "{}", show(&o));
}
