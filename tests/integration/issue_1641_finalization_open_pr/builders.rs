//! JSON builders, base context and shared assertions for the #1641 tests.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use super::fixture::{BRANCH, Fixture, LEAK_MARKER, Outcome, PR_URL};

pub fn check_run(status: &str, conclusion: Value) -> Value {
    json!({"__typename": "CheckRun", "name": "ci", "status": status, "conclusion": conclusion})
}

pub fn status_context(state: &str) -> Value {
    json!({"__typename": "StatusContext", "context": "legacy", "state": state})
}

pub fn green_rollup() -> Value {
    json!([check_run("COMPLETED", json!("SUCCESS"))])
}

/// A `gh pr view` body for PR #7 on the fixture repo; tweak fields per case.
pub fn open_pr(rollup: Value) -> Value {
    json!({
        "number": 7,
        "url": PR_URL,
        "state": "OPEN",
        "headRefName": BRANCH,
        "isCrossRepository": false,
        "statusCheckRollup": rollup,
    })
}

/// The base context of a resumed run: explicit repo path, the run branch, a
/// clean reporting step, and NO implementation/verification step outputs.
pub fn resumed_env(repo_path: &str) -> Vec<(&'static str, String)> {
    vec![
        ("REPO_PATH", repo_path.to_owned()),
        ("BRANCH_NAME", BRANCH.to_owned()),
        ("FINALIZER_STEP_STATUS", "ok".to_owned()),
    ]
}

pub fn as_refs<'a>(envs: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    envs.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

pub fn assert_failed_implementation(outcome: &Outcome, why: &str) {
    assert!(
        !outcome.success(),
        "{why}: must fail closed\n{}",
        outcome.describe()
    );
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_IMPLEMENTATION",
        "{why}\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_success"), "false", "{why}");
    assert!(
        outcome
            .stderr
            .contains("workflow finalization failed closed"),
        "{why}: stderr must announce the fail-closed result\n{}",
        outcome.describe()
    );
}

pub fn assert_no_leak(outcome: &Outcome) {
    for stream in [&outcome.stdout, &outcome.stderr] {
        assert!(
            !stream.contains(LEAK_MARKER) && !stream.contains("token-1641"),
            "gh stderr / credential-bearing text must never be emitted\n{}",
            outcome.describe()
        );
        assert!(
            !stream.contains("github.com/rysweet/fixture.git"),
            "the origin URL must never be emitted\n{}",
            outcome.describe()
        );
    }
}

pub fn path_has_retry_scratch(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".workflow-gh-retry")
        })
    })
}

pub fn set_branch(envs: &mut [(&'static str, String)], branch: &str) {
    let entry = envs
        .iter_mut()
        .find(|(k, _)| *k == "BRANCH_NAME")
        .expect("resumed_env sets BRANCH_NAME");
    entry.1 = branch.to_owned();
}

/// Proves the open-PR path really ran (one live `pr view 7`) and names why it
/// failed, so a guard that bailed out early cannot pass as the intended check.
pub fn assert_rejected_after_pr_view(fx: &Fixture, outcome: &Outcome, reason: &str, why: &str) {
    let views = fx
        .gh_calls()
        .into_iter()
        .filter(|c| c.starts_with("pr view 7 "))
        .count();
    assert_eq!(
        views,
        1,
        "{why}: exactly one live pr view\n{}",
        outcome.describe()
    );
    assert!(
        outcome.stderr.contains(reason),
        "{why}: stderr must name `{reason}`\n{}",
        outcome.describe()
    );
}
