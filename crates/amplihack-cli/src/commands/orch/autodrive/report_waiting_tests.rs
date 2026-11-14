//! #2472 (waits slice), decision A2 (no new stop reason): a loop that ended `WAITING`
//! is reported by the top-level reducer as `readiness: incomplete`,
//! `action: not_requested`, `stop_reason: blockers`, with the blocker
//! `waiting: <wait_target>`. Included from `report.rs` with:
//!
//! ```ignore
//! #[cfg(test)]
//! #[path = "report_waiting_tests.rs"]
//! mod waiting_tests;
//! ```
//!
//! `wait_target` is read with `as_str()` and checked against the closed
//! pattern used by `autodrive_round_evidence.sh`. An invalid value adds
//! `waiting: <invalid target>` and is never repeated; an absent value adds
//! no waiting blocker. The typed `StopReason` enum and the accepted-outcome
//! path do not change. Contract:
//! docs/reference/autodrive-github-reads.md#how-the-run-report-shows-waiting.

use super::report;
use crate::commands::orch::autodrive::authorization::InvocationAuthorization;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SHA: &str = "3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d";

fn auth() -> InvocationAuthorization {
    InvocationAuthorization::capture(
        "auto-drive-to-merge",
        &BTreeMap::from([
            ("no_merge".into(), "true".into()),
            ("autodrive_dry_run".into(), "false".into()),
        ]),
    )
    .expect("valid invocation")
}

/// Runs the reducer with no finalization evidence (the error path) and the
/// given loop result under `key`, passed as the JSON string the recipe
/// context holds.
fn reduce_with(key: &str, loop_result: Value) -> Value {
    let auth = auth();
    let context = json!({ key: loop_result.to_string() });
    report(
        &auth,
        context.as_object().unwrap(),
        None,
        std::path::Path::new("."),
    )
}

fn waiting_blockers(v: &Value) -> Vec<String> {
    v["blockers"]
        .as_array()
        .unwrap_or_else(|| panic!("blockers is a list: {v}"))
        .iter()
        .filter_map(Value::as_str)
        .filter(|b| b.starts_with("waiting:"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_waiting_merge_loop_is_incomplete_with_stop_reason_blockers_and_its_target() {
    for target in [
        format!("ci@{SHA}"),
        format!("mergeability@{SHA}"),
        "ratelimit-reset@1794528000".to_string(),
        "ratelimit-reset@unknown".to_string(),
        "read@pr-head".to_string(),
        "read@threads".to_string(),
    ] {
        let v = reduce_with(
            "merge_loop_result",
            json!({"loop_result": "WAITING", "loop": "merge-ready", "wait_reason": "ci-pending",
                   "wait_target": target}),
        );
        assert_eq!(v["readiness"], "incomplete", "{target}: {v}");
        assert_eq!(v["action"], "not_requested", "{target}: {v}");
        assert_eq!(v["stop_reason"], "blockers", "{target}: {v}");
        assert_eq!(waiting_blockers(&v), [format!("waiting: {target}")], "{v}");
    }
}

#[test]
fn a_waiting_crusty_loop_is_reported_the_same_way() {
    let v = reduce_with(
        "crusty_loop_result",
        json!({"loop_result": "WAITING", "loop": "crusty", "wait_reason": "head-unavailable",
               "wait_target": "read@pr-head"}),
    );
    assert_eq!(v["readiness"], "incomplete", "{v}");
    assert_eq!(v["stop_reason"], "blockers", "{v}");
    assert_eq!(waiting_blockers(&v), ["waiting: read@pr-head"], "{v}");
}

#[test]
fn a_waiting_loop_without_a_target_adds_no_waiting_blocker() {
    let v = reduce_with(
        "merge_loop_result",
        json!({"loop_result": "WAITING", "loop": "merge-ready", "wait_reason": "ci-pending"}),
    );
    assert_eq!(v["stop_reason"], "blockers", "{v}");
    assert_eq!(v["readiness"], "incomplete", "{v}");
    assert!(waiting_blockers(&v).is_empty(), "{v}");
}

#[test]
fn an_invalid_wait_target_is_named_invalid_and_never_repeated() {
    let forged = "ci@deadbeef; rm -rf / \u{1b}[31m";
    for target in [
        json!(forged),
        json!(format!("ci@{}", SHA.to_uppercase())),
        json!("ratelimit-reset@1234567890123"),
        json!("read@elsewhere"),
        json!(""),
        json!(42),
        json!({"target": format!("ci@{SHA}")}),
        json!([format!("ci@{SHA}")]),
    ] {
        let v = reduce_with(
            "merge_loop_result",
            json!({"loop_result": "WAITING", "loop": "merge-ready", "wait_target": target}),
        );
        assert_eq!(v["stop_reason"], "blockers", "{target}: {v}");
        assert_eq!(
            waiting_blockers(&v),
            ["waiting: <invalid target>"],
            "{target}: {v}"
        );
        let text = v.to_string();
        assert!(
            !text.contains("rm -rf"),
            "the forged value is not repeated: {text}"
        );
        assert!(!text.contains("deadbeef"), "{text}");
        assert!(!text.contains("\\u001b"), "{text}");
    }
}

#[test]
fn a_waiting_result_never_makes_readiness_ready_or_requests_an_action() {
    let v = reduce_with(
        "merge_loop_result",
        json!({"loop_result": "WAITING", "wait_target": format!("ci@{SHA}")}),
    );
    assert_ne!(v["readiness"], "ready", "{v}");
    assert_eq!(v["action"], "not_requested", "{v}");
    assert_eq!(v["platform_state"], "unknown", "{v}");
}

#[test]
fn the_existing_stop_reasons_are_unchanged() {
    for (result, reason) in [
        ("BUDGET_EXHAUSTED", "budget_exhausted"),
        ("USAGE_LIMIT", "usage_limit"),
        ("ORPHANED", "orphaned"),
        ("TERMINAL_POLICY_REFUSAL", "terminal_refusal"),
        ("STUCK", "no_progress"),
        ("SOMETHING_ELSE", "dependency_failure"),
    ] {
        let v = reduce_with(
            "merge_loop_result",
            json!({"loop_result": result, "wait_target": format!("ci@{SHA}")}),
        );
        assert_eq!(v["stop_reason"], reason, "{result}: {v}");
        assert!(
            waiting_blockers(&v).is_empty(),
            "only WAITING adds a waiting blocker: {result}: {v}"
        );
    }
}
