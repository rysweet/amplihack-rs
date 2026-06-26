//! Regression tests for issues #815 and #804 — default-workflow local tracking
//! issue-number extraction.
//!
//! ## Problem
//!
//! When `step-03-create-issue` falls back to local tracking it emits a
//! hash-based reference (`local-<hash>`, e.g. `local-5d904cff4398`) instead of
//! a numeric GitHub issue number. `step-03b-extract-issue-number` previously
//! dropped that reference to an empty issue_number (or, in earlier revisions,
//! hard-failed), losing traceability — and on malformed metadata it aborted the
//! workflow after classification/analysis had already succeeded.
//!
//! ## Contract verified here
//!
//! `step-03b-extract-issue-number` must:
//!   * propagate a well-formed local tracking reference (`local-<hash>`,
//!     `local-issue-<n>`, legacy `local-tracking:<n>`) verbatim downstream,
//!   * never surface the bare embedded number for a local fallback (a derived
//!     number must not become a `Closes #N` closing an unrelated issue),
//!   * still extract real numeric issue/work-item numbers for GitHub/AzDO, and
//!   * still fail closed (with sanitization) for genuinely unparseable output
//!     and for malformed local metadata that carries markers but no reference.
//!
//! The tests execute the real bash body extracted from the recipe YAML — they
//! do not re-implement the logic — so they stay coupled to the shipped recipe.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_yaml::Value;

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bins/amplihack -> bins
    p.pop(); // bins -> workspace root
    p
}

/// Extract the bash `command:` body of `step-03b-extract-issue-number` from the
/// shipped `workflow-prep.yaml` recipe.
fn step_03b_body() -> String {
    let path = workspace_root().join("amplifier-bundle/recipes/workflow-prep.yaml");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let recipe: Value =
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("workflow-prep.yaml must have a top-level 'steps' sequence");
    for step in steps {
        if step.get("id").and_then(Value::as_str) == Some("step-03b-extract-issue-number") {
            return step
                .get("command")
                .and_then(Value::as_str)
                .expect("step-03b-extract-issue-number must define a 'command' body")
                .to_owned();
        }
    }
    panic!("step-03b-extract-issue-number not found in workflow-prep.yaml");
}

fn run_step_03b(issue_creation: &str, task_description: &str) -> Output {
    Command::new("bash")
        .arg("-c")
        .arg(step_03b_body())
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("ISSUE_CREATION", issue_creation)
        .env("TASK_DESCRIPTION", task_description)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run step-03b-extract-issue-number")
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// The exact failing shape reported in issues #815 and #804: hash-based local
/// metadata with no numeric issue number anywhere.
#[test]
fn hash_based_local_reference_is_accepted_and_propagated() {
    let out = run_step_03b(
        "tracking_system=local\ntracking_reference=local-5d904cff4398\ntracking_issue=local-5d904cff4398\nissue_creation=local-tracking\n",
        "update PR !598 guide and start stacked XPIA implementation workstreams",
    );
    assert!(
        out.status.success(),
        "step-03b must not abort on a hash-based local reference; stderr:\n{}",
        stderr_of(&out)
    );
    assert_eq!(
        stdout_of(&out),
        "local-5d904cff4398",
        "step-03b must propagate the hash-based local tracking reference downstream"
    );
}

/// `local-issue-763` carries `issue_number=763`, but a local fallback must NOT
/// surface the bare `763` (it could become a `Closes #763` closing an unrelated
/// issue). It must propagate the non-numeric reference instead.
#[test]
fn local_tracking_never_surfaces_bare_number() {
    let out = run_step_03b(
        "tracking_system=local\ntracking_reference=local-issue-763\ntracking_issue=local-issue-763\nissue_creation=local-tracking\nissue_number=763\n",
        "Create tracking for local fallback",
    );
    assert!(out.status.success(), "stderr:\n{}", stderr_of(&out));
    assert_eq!(stdout_of(&out), "local-issue-763");
    assert_ne!(
        stdout_of(&out),
        "763",
        "local tracking must not surface a bare numeric issue id"
    );
}

/// Legacy `local-tracking:<n>` references must be propagated verbatim too.
#[test]
fn legacy_local_tracking_colon_reference_is_propagated() {
    let out = run_step_03b(
        "tracking_reference=local-tracking:123\ntracking_issue=local-tracking:123\n",
        "legacy local tracking shape",
    );
    assert!(out.status.success(), "stderr:\n{}", stderr_of(&out));
    assert_eq!(stdout_of(&out), "local-tracking:123");
}

/// Regression guard: real GitHub issue numbers must still extract numerically.
#[test]
fn numeric_github_issue_number_still_extracts() {
    let out = run_step_03b(
        "https://github.com/example-org/example-repo/issues/901",
        "GitHub follow-up",
    );
    assert!(out.status.success(), "stderr:\n{}", stderr_of(&out));
    assert_eq!(
        stdout_of(&out),
        "901",
        "real GitHub issue numbers must still extract numerically"
    );
}

/// Regression guard: real AzDO work-item numbers must still extract numerically.
#[test]
fn azdo_work_item_number_still_extracts() {
    let out = run_step_03b(
        "https://dev.azure.com/org/proj/_workitems/edit/4242",
        "AzDO follow-up",
    );
    assert!(out.status.success(), "stderr:\n{}", stderr_of(&out));
    assert_eq!(stdout_of(&out), "4242");
}

/// Malformed local metadata (markers present, no valid `local-*` reference) must
/// keep failing loud rather than fabricating an issue number — the local-ref
/// acceptance must not weaken this deliberate fail-closed guard.
#[test]
fn malformed_local_metadata_without_reference_fails_closed() {
    let out = run_step_03b(
        "tracking_system=local\nissue_creation=local-tracking\nissue_number=763\n",
        "Fallback with no usable reference",
    );
    assert!(
        !out.status.success(),
        "malformed local metadata must fail closed; stdout:\n{}",
        stdout_of(&out)
    );
    assert_eq!(
        stdout_of(&out),
        "",
        "malformed local metadata must not emit a fabricated issue number"
    );
    assert!(
        stderr_of(&out).contains("local tracking metadata missing valid local reference"),
        "fail-closed diagnostic must be preserved; stderr:\n{}",
        stderr_of(&out)
    );
}

/// Genuinely unparseable, non-local output must still fail closed and sanitize
/// any credential-bearing text.
#[test]
fn unparseable_non_local_output_still_fails_closed() {
    let out = run_step_03b(
        "GraphQL: rate limit exceeded for https://token:ghp_secret123@github.com/example-org/example-repo",
        "Create tracking with no usable reference",
    );
    assert!(
        !out.status.success(),
        "non-local unparseable output must still fail closed; stdout:\n{}",
        stdout_of(&out)
    );
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("ERROR: step-03b failed to extract issue number"),
        "hard-fail must keep its diagnostic; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("ghp_secret123"),
        "credential-bearing issue_creation output must be sanitized; stderr:\n{stderr}"
    );
}
