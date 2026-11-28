//! Issue #3440. The CI wait ends at once with `base-conflict` when GitHub
//! states that the pull request conflicts with its base, and keeps waiting
//! while the mergeable state is unknown. The sleep stand-in advances the
//! scenario without elapsed time.

#![cfg(target_os = "linux")]

use super::ci_wait::{fake_sleep, read_and_decide, sleeps, wait};
use super::support::Harness;
use serde_json::{Value, json};
use std::fs;

#[test]
fn a_conflicting_pr_without_required_checks_ends_the_wait_on_the_first_read() {
    let h = Harness::new();
    h.set_checks("");
    let reading = read_and_decide(&h, false);
    h.rest_mergeable(json!(false), "dirty");
    fs::remove_file(h.gh.join("calls.log")).unwrap();
    fake_sleep(&h, "exit 1");
    let result = wait(&h, "37");
    assert_eq!(result["outcome"], "base-conflict", "{result}");
    assert_eq!(result["head"], reading["pr"]["head"]);
    assert_eq!(result["base"], "main");
    assert_eq!(result["base_sha"], h.base_head());
    assert!(sleeps(&h).is_empty(), "{:?}", sleeps(&h));
    let calls = h.calls();
    assert!(
        calls.iter().any(|c| c.contains("repos/o/r/pulls/7")),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.contains("/check-runs?")),
        "the wait ended before it read the checks: {calls:?}"
    );
}

#[test]
fn an_unknown_mergeable_state_keeps_the_wait_going() {
    let h = Harness::new();
    h.set_checks("build\tpending\t0\n");
    read_and_decide(&h, false);
    h.rest_mergeable(Value::Null, "unknown");
    fake_sleep(
        &h,
        r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
printf 'build\tpass\t0\n' > "$GH_STATE/checks""#,
    );
    let result = wait(&h, "37");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "passed", "{result}");
    assert_eq!(sleeps(&h), [37]);
}

#[test]
fn a_clean_pr_with_pending_checks_waits_as_before() {
    let h = Harness::new();
    h.set_checks("build\tpending\t0\n");
    read_and_decide(&h, false);
    h.rest_mergeable(json!(true), "blocked");
    fake_sleep(
        &h,
        r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
printf 'build\tpass\t0\n' > "$GH_STATE/checks""#,
    );
    let result = wait(&h, "37");
    assert_eq!(result["outcome"], "checks-complete", "{result}");
    assert_eq!(result["checks_state"], "passed", "{result}");
    assert_eq!(sleeps(&h), [37]);
}

#[test]
fn a_conflict_stated_on_a_later_read_ends_the_wait() {
    let h = Harness::new();
    h.set_checks("build\tpending\t0\n");
    read_and_decide(&h, false);
    h.rest_mergeable(json!(true), "clean");
    fake_sleep(
        &h,
        r#"test "$(wc -l < "$GH_STATE/sleeps")" -eq 1
printf '%s\n' '{"mergeable":false,"mergeable_state":"dirty"}' > "$GH_STATE/rest-mergeable""#,
    );
    let result = wait(&h, "37");
    assert_eq!(result["outcome"], "base-conflict", "{result}");
    assert_eq!(result["base_sha"], h.base_head());
    assert_eq!(sleeps(&h), [37]);
}
