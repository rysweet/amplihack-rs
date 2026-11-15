//! #2663: the run's final status follows GitHub's merge state in both
//! directions.
//!
//! Three drives merged their PR, then a later step failed (the readiness
//! reducer compared against a base their own merge had moved) and the run
//! ended Failed. Merged at the expected head is a success whatever happens
//! after it; not merged is never a success. Each test starts from the run
//! result the runner hands back, runs `report()` and then
//! `apply_autodrive_outcome`, as `execute.rs` does.
//! docs/reference/autodrive-merge-ready-round.md#success-means-merged.

use super::RecipeRunResult;
use super::execute::apply_autodrive_outcome;
use crate::commands::orch::autodrive::{authorization::InvocationAuthorization, report::report};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

const PR: u64 = 2663;
const HEAD: &str = concat!("15a7f6f47c1e2d0b9a8f", "7e6d5c4b3a2918070605");

fn authorization() -> InvocationAuthorization {
    InvocationAuthorization::capture(
        "auto-drive-to-merge",
        &BTreeMap::from([
            ("no_merge".into(), "false".into()),
            ("autodrive_dry_run".into(), "false".into()),
        ]),
    )
    .expect("valid invocation")
}

fn finalize_merge(result: &str) -> Value {
    Value::String(
        json!({"result": result, "pr": PR, "expected_head": HEAD, "handoff": "false"}).to_string(),
    )
}

/// The runner's result and `report()`'s outcome after `apply_autodrive_outcome`.
fn final_status(
    auth: &InvocationAuthorization,
    runner_success: bool,
    context: Map<String, Value>,
) -> (bool, Value) {
    let mut run_result = RecipeRunResult {
        recipe_name: "auto-drive-to-merge".into(),
        success: runner_success,
        context,
        ..RecipeRunResult::default()
    };
    let outcome = report(auth, &run_result.context, Some(&PR.to_string()));
    apply_autodrive_outcome(&mut run_result, &outcome);
    (run_result.success, outcome)
}

fn context(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

/// #2663's case at runner level: the merge moved the base, a later step
/// failed, and no loop result was recorded. The run is a success.
#[test]
fn merged_then_later_step_failed_is_success() {
    let auth = authorization();
    let ctx = context(&[("finalize_merge", finalize_merge("merged"))]);
    let (success, outcome) = final_status(&auth, false, ctx);
    assert!(success, "{outcome}");
    assert_eq!(outcome["stop_reason"], json!("completed"), "{outcome}");
    assert!(
        outcome["note"]
            .as_str()
            .is_some_and(|n| n.contains("loop_result not consulted")),
        "{outcome}"
    );
}

#[test]
fn merged_with_an_unreadable_loop_result_is_success() {
    let auth = authorization();
    let ctx = context(&[
        ("merge_loop_result", Value::String("not json".into())),
        ("finalize_merge", finalize_merge("merged")),
    ]);
    let (success, outcome) = final_status(&auth, false, ctx);
    assert!(success, "{outcome}");
    assert_eq!(outcome["stop_reason"], json!("completed"), "{outcome}");
}

#[test]
fn refused_then_later_step_failed_is_failure() {
    let auth = authorization();
    let ctx = context(&[("finalize_merge", finalize_merge("refused"))]);
    let (success, outcome) = final_status(&auth, false, ctx);
    assert!(!success, "{outcome}");
    assert_ne!(outcome["stop_reason"], json!("completed"), "{outcome}");
}

/// The reverse direction: the runner said success, but the PR did not merge.
#[test]
fn base_behind_with_success_runner_is_failure() {
    let auth = authorization();
    let loop_line = json!({"loop": "merge-ready", "loop_result": "DONE"}).to_string();
    let ctx = context(&[
        ("merge_loop_result", Value::String(loop_line)),
        ("finalize_merge", finalize_merge("base-behind")),
    ]);
    let (success, outcome) = final_status(&auth, true, ctx);
    assert!(!success, "{outcome}");
    assert_eq!(outcome["stop_reason"], json!("not_merged"), "{outcome}");
}

/// Without the expected head the record does not say "merged at the expected
/// head", so it does not raise a failed run.
#[test]
fn merged_without_expected_head_does_not_raise() {
    let auth = authorization();
    let merged = json!({"result": "merged", "pr": PR, "handoff": "false"}).to_string();
    let ctx = context(&[("finalize_merge", Value::String(merged))]);
    let (success, outcome) = final_status(&auth, false, ctx);
    assert!(!success, "{outcome}");
    assert_ne!(outcome["stop_reason"], json!("completed"), "{outcome}");
}

/// A merge record with no `pr` is not trusted when a PR is expected.
#[test]
fn merged_without_pr_does_not_raise() {
    let auth = authorization();
    let merged = json!({"result": "merged", "expected_head": HEAD}).to_string();
    let ctx = context(&[("finalize_merge", Value::String(merged))]);
    let (success, outcome) = final_status(&auth, false, ctx);
    assert!(!success, "{outcome}");
    assert!(outcome["merge"].is_null(), "{outcome}");
}

/// #2329: a merge record next to a state directory other than the
/// runner-assigned one is ignored, so it raises nothing.
#[test]
fn merged_with_wrong_state_dir_does_not_raise() {
    let dir = tempfile::tempdir().unwrap();
    let mut auth = authorization();
    auth.bind_state_dir(dir.path().join("assigned"));
    let ctx = context(&[
        ("finalize_merge", finalize_merge("merged")),
        ("autodrive_state_dir", json!(dir.path().join("other"))),
    ]);
    let (success, outcome) = final_status(&auth, false, ctx);
    assert!(!success, "{outcome}");
    assert!(outcome["merge"].is_null(), "{outcome}");
}

/// #2575 B4: a verify-only run that reaches DONE is a success, as before
/// 8794c2df2 (docs/howto/verify-pr-with-auto-drive.md). Only a merge run
/// must merge to succeed.
#[test]
fn verify_only_run_that_reaches_done_is_success() {
    for (no_merge, dry_run) in [("true", "false"), ("false", "true")] {
        let auth = InvocationAuthorization::capture(
            "auto-drive-light",
            &BTreeMap::from([
                ("no_merge".into(), no_merge.into()),
                ("autodrive_dry_run".into(), dry_run.into()),
            ]),
        )
        .expect("valid invocation");
        let loop_line = json!({"loop": "merge-ready", "loop_result": "DONE"}).to_string();
        let ctx = context(&[
            ("merge_loop_result", Value::String(loop_line)),
            ("finalize_merge", finalize_merge("not-authorised")),
        ]);
        let (success, outcome) = final_status(&auth, false, ctx);
        assert!(success, "{outcome}");
        assert_eq!(outcome["stop_reason"], json!("completed"), "{outcome}");
        assert_eq!(outcome["requested_action"], json!("verify_readiness"));
    }
}

/// The same result on a merge run is a refusal, never a success.
#[test]
fn not_authorised_on_a_merge_run_is_failure() {
    let auth = authorization();
    let loop_line = json!({"loop": "merge-ready", "loop_result": "DONE"}).to_string();
    let ctx = context(&[
        ("merge_loop_result", Value::String(loop_line)),
        ("finalize_merge", finalize_merge("not-authorised")),
    ]);
    let (success, outcome) = final_status(&auth, true, ctx);
    assert!(!success, "{outcome}");
    assert_eq!(outcome["stop_reason"], json!("not_merged"), "{outcome}");
}
