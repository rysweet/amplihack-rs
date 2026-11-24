//! #2663 M1: an auto-drive run succeeds only when it merged.
//!
//! `autodrive_run_succeeded(&autodrive_outcome)` is an allow-list of the one
//! `stop_reason` value `completed` (merged in this run, already merged at
//! entry, or a merging-off run that ended ready). The runner applies it after
//! `report()` adds `autodrive_outcome`, after `run_with_transient_retry` has
//! returned, so a run that did not merge never starts the whole-run
//! re-attempt (#1267).
//! docs/reference/autodrive-goal-loop.md#reported-outcomes.

use super::execute::autodrive_run_succeeded;
use serde_json::json;

#[test]
fn completed_is_the_only_success() {
    assert!(autodrive_run_succeeded(&json!({
        "schema_version": 2,
        "stop_reason": "completed",
        "merge": {"result": "merged", "pr": 2575}
    })));
    assert!(autodrive_run_succeeded(&json!({
        "stop_reason": "completed",
        "merge": {"result": "already-merged", "pr": 2575}
    })));
}

#[test]
fn every_other_stop_reason_is_a_failure() {
    for reason in [
        "not_merged",
        "waiting_for_checks",
        "handoff",
        "dependency_failure",
        "unreadable",
        "budget_exhausted",
        "usage_limit",
        "orphaned",
        "terminal_refusal",
        "no_progress",
        // #3129: the goal loop's other endings.
        "closed",
        "stopped",
        "failed",
        "",
        "bogus",
        "Completed",
        " completed",
        "completed\n",
    ] {
        assert!(
            !autodrive_run_succeeded(&json!({"stop_reason": reason})),
            "{reason:?} is not a success"
        );
    }
}

#[test]
fn a_missing_or_odd_stop_reason_is_a_failure() {
    for outcome in [
        json!({}),
        json!(null),
        json!("completed"),
        json!(["completed"]),
        json!({"stop_reason": null}),
        json!({"stop_reason": true}),
        json!({"stop_reason": ["completed"]}),
        json!({"merge": {"result": "merged"}}),
    ] {
        assert!(!autodrive_run_succeeded(&outcome), "{outcome}");
    }
}
