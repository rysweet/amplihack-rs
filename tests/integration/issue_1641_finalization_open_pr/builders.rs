//! JSON builders, base context and shared assertions for the #1641 tests.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

use super::fixture::{BRANCH, Fixture, LEAK_MARKER, Outcome, PR_URL, tool_path};

/// The answer `assess-finalization` wrote, as the recipe stores it (#2343,
/// #2809). Since the assessment decides the terminal state, a validator test
/// that expects one must supply the decision that produced it; a test with no
/// assessment is a test of what finalization does when nothing decided.
pub fn assessment(terminal_state: &str, action: &str, pr: &str) -> (&'static str, String) {
    (
        "RECIPE_VAR_finalization_assessment",
        json!({
            "terminal_state": terminal_state,
            "action_taken": action,
            "pr_number": pr,
            "reason": format!("assess-finalization decided {terminal_state} on the run's outcome"),
            "next_action": if pr.is_empty() {
                format!("Act on {terminal_state}.")
            } else {
                format!("Act on {terminal_state} for PR #{pr}.")
            },
        })
        .to_string(),
    )
}

/// The same answer, exactly as written. `assessment` renders every leaf as a
/// string; this takes the object so a case can write a leaf the way a model
/// actually would -- `gh` reports `"number": 7`, and an agent asked to report
/// the pull request it read back copies the integer (#2819).
pub fn assessment_raw(answer: Value) -> (&'static str, String) {
    ("RECIPE_VAR_finalization_assessment", answer.to_string())
}

/// Collected evidence in the canonical `RECIPE_VAR_finalization_evidence`
/// transport, the shape `collect-finalization-evidence` writes. A run that
/// carries any `RECIPE_VAR_*` is on the canonical context tier, where the flat
/// legacy aliases are not read; a test that supplies an assessment must
/// therefore supply its evidence this way too.
pub fn evidence_env(
    dirty_worktree: &str,
    tooling_missing: &str,
    hollow_success: &str,
) -> (&'static str, String) {
    (
        "RECIPE_VAR_finalization_evidence",
        json!({
            "schema_version": 1,
            "git": {"dirty_worktree": dirty_worktree},
            "tooling": {"missing": tooling_missing, "gh_required": "false"},
            "prior_terminal_state": {"terminal_state": ""},
            "agent_outputs": {"hollow_success_signals": hollow_success},
        })
        .to_string(),
    )
}

/// `assessment` plus the run context of a resumed run.
pub fn assessed_env(
    repo_path: &str,
    terminal_state: &str,
    pr: &str,
) -> Vec<(&'static str, String)> {
    let mut envs = resumed_env(repo_path);
    envs.push(assessment(terminal_state, "none", pr));
    envs
}

/// The result when nothing decided the terminal state: finalization reports
/// that it has none and leaves the step failing for its recovery agent.
pub fn assert_no_decision(outcome: &Outcome, why: &str) {
    assert!(
        !outcome.success(),
        "{why}: must fail\n{}",
        outcome.describe()
    );
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_MISSING_TERMINAL_EVIDENCE",
        "{why}\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_success"), "false", "{why}");
}

pub fn check_run(status: &str, conclusion: Value) -> Value {
    check_run_named("ci", status, conclusion)
}

/// A check run under its own name, for cases that assert which check the
/// reader reported.
pub fn check_run_named(name: &str, status: &str, conclusion: Value) -> Value {
    json!({"__typename": "CheckRun", "name": name, "status": status, "conclusion": conclusion})
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

impl Fixture {
    /// Call `finalization_outcome_facts <dir> <branch> [<ref>]` directly and
    /// parse its one JSON object. The facts reader is where the rules for
    /// reading a pull request live since #2343; the validator only reports the
    /// terminal state the assessment decided on what this returns.
    pub fn facts(&self, branch: &str, pr_ref: Option<&str>) -> (Value, Outcome) {
        self.facts_for_host(branch, pr_ref, "")
    }

    /// The same, for a run that recorded `host_type` as its remote (#1641 D5).
    pub fn facts_for_host(
        &self,
        branch: &str,
        pr_ref: Option<&str>,
        host_type: &str,
    ) -> (Value, Outcome) {
        let mut command = Command::new("bash");
        self.isolate(&mut command);
        command
            .arg("-c")
            .arg(
                r#"set -u; . "$1"; . "$2"; . "$3"; . "$4"; shift 4; finalization_outcome_facts "$@""#,
            )
            .arg("facts")
            .arg(tool_path("workflow_github_repo.sh"))
            .arg(tool_path("workflow_gh_retry.sh"))
            .arg(tool_path("workflow_finalization_pr_signal.sh"))
            .arg(tool_path("workflow_finalization_outcome.sh"))
            .arg(&self.repo)
            .arg(branch);
        command.arg(pr_ref.unwrap_or("")).arg(host_type);
        let output = command.output().expect("run finalization_outcome_facts");
        let outcome = Outcome {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            output,
        };
        let value: Value = serde_json::from_str(outcome.stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "the facts reader must print one JSON object: {e}\n{}",
                outcome.describe()
            )
        });
        (value, outcome)
    }

    /// One fact, as a String. Every field of the facts object is a String.
    pub fn fact(&self, branch: &str, pr_ref: Option<&str>, key: &str) -> String {
        self.facts(branch, pr_ref).0[key]
            .as_str()
            .unwrap_or_else(|| panic!("fact `{key}` must be a String"))
            .to_owned()
    }

    /// Call `finalization_open_pr_signal <dir> <branch> [<ref>]` directly.
    pub fn signal(&self, branch: &str, pr_ref: Option<&str>) -> Outcome {
        let mut command = Command::new("bash");
        self.isolate(&mut command);
        command
            .arg("-c")
            .arg(r#"set -u; . "$1"; . "$2"; shift 2; finalization_open_pr_signal "$@""#)
            .arg("signal")
            .arg(tool_path("workflow_gh_retry.sh"))
            .arg(tool_path("workflow_finalization_pr_signal.sh"))
            .arg(&self.repo)
            .arg(branch);
        if let Some(pr_ref) = pr_ref {
            command.arg(pr_ref);
        }
        let output = command.output().expect("run finalization_open_pr_signal");
        Outcome {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            output,
        }
    }
}
