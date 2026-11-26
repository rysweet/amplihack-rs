//! #2663: the run's final status follows GitHub's merge state in both
//! directions.
//!
//! Three drives merged their PR, then a later step failed (the readiness
//! reducer compared against a base their own merge had moved) and the run
//! ended Failed. Merged is a success whatever happens after it; not merged
//! is never a success. Each test starts from the run result the runner hands
//! back, runs `report()` and then `apply_autodrive_outcome`, as `execute.rs`
//! does.
//!
//! #3129: "merged" is the loop driver's `loop-end.json` with `end` `merged`,
//! written after its own reading of GitHub, in the runner-bound state folder.
//! The merge action's `merge-result.json` beside it fills the report's
//! `merge` field and never raises a run by itself.
//! docs/reference/autodrive-goal-loop.md#reported-outcomes.

use super::RecipeRunResult;
use super::execute::apply_autodrive_outcome;
use crate::commands::orch::autodrive::{
    authorization::InvocationAuthorization, report::report, state::EvidenceStore,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::os::unix::fs::OpenOptionsExt;

const PR: u64 = 2663;
const HEAD: &str = concat!("15a7f6f47c1e2d0b9a8f", "7e6d5c4b3a2918070605");

/// An invocation with a private state folder bound to it, as the runner
/// prepares a drive.
struct Drive {
    _temp: tempfile::TempDir,
    auth: InvocationAuthorization,
}

fn drive(recipe: &str, no_merge: &str, dry_run: &str) -> Drive {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("state");
    EvidenceStore::create(&dir).unwrap();
    let mut auth = InvocationAuthorization::capture(
        recipe,
        &BTreeMap::from([
            ("no_merge".into(), no_merge.into()),
            ("autodrive_dry_run".into(), dry_run.into()),
        ]),
    )
    .expect("valid invocation");
    auth.bind_state_dir(dir);
    Drive { _temp: temp, auth }
}

fn merge_drive() -> Drive {
    drive("auto-drive-to-merge", "false", "false")
}

impl Drive {
    fn put(&self, name: &str, body: &Value) {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.auth.state_dir().unwrap().join(name))
            .unwrap();
        std::io::Write::write_all(&mut file, format!("{body}\n").as_bytes()).unwrap();
    }
    /// The loop driver's end record for this run.
    fn end(&self, word: &str) {
        self.put(
            "loop-end.json",
            &json!({"end": word, "run_id": self.auth.run_id(), "subject": "PR 2663", "detail": "fixture"}),
        );
    }
    /// The merge action's record for this run.
    fn merge(&self, result: &str) {
        self.put(
            "merge-result.json",
            &json!({"result": result, "pr": PR, "expected_head": HEAD, "handoff": "false", "run_id": self.auth.run_id()}),
        );
    }
    /// The runner's result and `report()`'s outcome after
    /// `apply_autodrive_outcome`.
    fn final_status(&self, runner_success: bool, context: Map<String, Value>) -> (bool, Value) {
        let mut run_result = RecipeRunResult {
            recipe_name: "auto-drive-to-merge".into(),
            success: runner_success,
            context,
            ..RecipeRunResult::default()
        };
        let outcome = report(&self.auth, &run_result.context, Some(&PR.to_string()));
        apply_autodrive_outcome(&mut run_result, &outcome);
        (run_result.success, outcome)
    }
}

fn context(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

/// #2663's case at runner level: the merge moved the base and a later step
/// failed. The run is a success.
#[test]
fn merged_then_later_step_failed_is_success() {
    let drive = merge_drive();
    drive.merge("merged");
    drive.end("merged");
    let (success, outcome) = drive.final_status(false, Map::new());
    assert!(success, "{outcome}");
    assert_eq!(outcome["stop_reason"], json!("completed"), "{outcome}");
    assert_eq!(outcome["merge"]["result"], json!("merged"), "{outcome}");
}

#[test]
fn merged_with_an_unreadable_loop_result_is_success() {
    let drive = merge_drive();
    drive.merge("merged");
    drive.end("merged");
    let ctx = context(&[("merge_loop_result", Value::String("not json".into()))]);
    let (success, outcome) = drive.final_status(false, ctx);
    assert!(success, "{outcome}");
    assert_eq!(outcome["stop_reason"], json!("completed"), "{outcome}");
}

#[test]
fn refused_then_later_step_failed_is_failure() {
    let drive = merge_drive();
    drive.merge("refused");
    drive.end("handoff");
    let (success, outcome) = drive.final_status(false, Map::new());
    assert!(!success, "{outcome}");
    assert_ne!(outcome["stop_reason"], json!("completed"), "{outcome}");
}

/// The reverse direction: the runner said success, but the PR did not merge.
#[test]
fn handoff_with_success_runner_is_failure() {
    let drive = merge_drive();
    drive.merge("checks-not-green");
    drive.end("handoff");
    let (success, outcome) = drive.final_status(true, Map::new());
    assert!(!success, "{outcome}");
    assert_eq!(outcome["stop_reason"], json!("handoff"), "{outcome}");
}

/// A merge record alone, even `merged` at the expected head, does not say
/// the loop ended merged, so it does not raise a failed run.
#[test]
fn merged_record_without_a_merged_end_does_not_raise() {
    let drive = merge_drive();
    drive.merge("merged");
    let (success, outcome) = drive.final_status(false, Map::new());
    assert!(!success, "{outcome}");
    assert_ne!(outcome["stop_reason"], json!("completed"), "{outcome}");
}

/// Without the expected head the record does not say "merged at the expected
/// head", so it does not raise a failed run.
#[test]
fn merged_without_expected_head_does_not_raise() {
    let drive = merge_drive();
    drive.put(
        "merge-result.json",
        &json!({"result": "merged", "pr": PR, "handoff": "false", "run_id": drive.auth.run_id()}),
    );
    let (success, outcome) = drive.final_status(false, Map::new());
    assert!(!success, "{outcome}");
    assert_ne!(outcome["stop_reason"], json!("completed"), "{outcome}");
}

/// A merge record with no `pr` is not trusted when a PR is expected.
#[test]
fn merged_without_pr_does_not_raise() {
    let drive = merge_drive();
    drive.put(
        "merge-result.json",
        &json!({"result": "merged", "expected_head": HEAD, "run_id": drive.auth.run_id()}),
    );
    let (success, outcome) = drive.final_status(false, Map::new());
    assert!(!success, "{outcome}");
    assert!(outcome["merge"].is_null(), "{outcome}");
}

/// #2329: records beside a state directory other than the runner-assigned
/// one are ignored, so they raise nothing.
#[test]
fn merged_with_wrong_state_dir_does_not_raise() {
    let drive = merge_drive();
    drive.merge("merged");
    drive.end("merged");
    let other = drive.auth.state_dir().unwrap().with_file_name("other");
    let ctx = context(&[("autodrive_state_dir", json!(other))]);
    let (success, outcome) = drive.final_status(false, ctx);
    assert!(!success, "{outcome}");
    assert!(outcome["merge"].is_null(), "{outcome}");
}

/// #2575 B4: a verify-only run that reaches its readiness end is a success
/// (docs/howto/verify-pr-with-auto-drive.md). Only a merge run must merge to
/// succeed.
#[test]
fn verify_only_run_that_reaches_ready_is_success() {
    for (no_merge, dry_run) in [("true", "false"), ("false", "true")] {
        let drive = drive("auto-drive-light", no_merge, dry_run);
        drive.merge("not-authorised");
        drive.end("ready");
        let (success, outcome) = drive.final_status(false, Map::new());
        assert!(success, "{outcome}");
        assert_eq!(outcome["stop_reason"], json!("completed"), "{outcome}");
        assert_eq!(outcome["requested_action"], json!("verify_readiness"));
    }
}

/// The same end on a merge run is not a success: merging was on, so `ready`
/// is a hand-off, never a success.
#[test]
fn not_authorised_on_a_merge_run_is_failure() {
    let drive = merge_drive();
    drive.merge("not-authorised");
    drive.end("ready");
    let (success, outcome) = drive.final_status(true, Map::new());
    assert!(!success, "{outcome}");
    assert_eq!(outcome["stop_reason"], json!("handoff"), "{outcome}");
    assert_eq!(outcome["merge"]["result"], json!("not-authorised"));
}
