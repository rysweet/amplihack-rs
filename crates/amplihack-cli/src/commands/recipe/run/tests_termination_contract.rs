//! The decoder against the pinned runner crate itself (PR #1442 review,
//! round 4).
//!
//! `tests_termination.rs` rebuilds the runner's envelopes with copied format
//! strings. A pin bump that changed the runner's text would leave those tests
//! green while `termination_exit_code` returned `None`, and
//! `structural_refusal` would go back to `false` with nothing failing. The
//! tests here take the text from `recipe_runner_rs` at the pinned revision:
//!
//! - the outer `Step '<id>' failed: ` envelope from
//!   `models::StepExecutionError`'s own `Display`, with an id that needs
//!   escaping (the part that changed between `4d5187e` and `90286bc`);
//! - the `Command failed (exit N): <stderr>` text from
//!   `adapters::cli_subprocess::CLISubprocessAdapter::execute_bash_step`,
//!   running a real `exit 79`.
//!
//! Two pieces are still copied, because the runner builds them in private
//! code: the `bash step failed: {:#}` wrapper (`runner/mod.rs`, the
//! `StepType::Bash` branch) and the sub-recipe header
//! (`describe_sub_recipe_failure`). Issue #2490, a numeric exit code from the
//! runner, replaces the decoder and both copies.

use std::collections::HashMap;

use recipe_runner_rs::adapters::Adapter;
use recipe_runner_rs::adapters::cli_subprocess::CLISubprocessAdapter;
use recipe_runner_rs::models::StepExecutionError;

use super::super::RecipeRunStepResult;
use super::tests_termination::sub_recipe;

fn decode(step_id: &str, error: &str) -> Option<i32> {
    RecipeRunStepResult {
        step_id: step_id.to_string(),
        status: "failed".to_string(),
        error: error.to_string(),
        ..RecipeRunStepResult::default()
    }
    .termination_exit_code()
}

/// The pinned runner's error for a bash step that runs `script`: the adapter's
/// own failure, wrapped the way the runner wraps it.
fn real_bash_failure(step_id: &str, script: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let failure = CLISubprocessAdapter::new()
        .execute_bash_step(
            script,
            dir.path().to_str().expect("utf-8 tempdir"),
            Some(60),
            &HashMap::new(),
        )
        .expect_err("the script exits non-zero");
    StepExecutionError {
        step_id: step_id.to_string(),
        message: format!("bash step failed: {failure:#}"),
    }
    .to_string()
}

#[test]
fn the_pinned_step_execution_error_decodes_as_79() {
    let error = StepExecutionError {
        step_id: "a'b".to_string(),
        message: "bash step failed: Command failed (exit 79): x".to_string(),
    }
    .to_string();
    assert_eq!(decode("a'b", &error), Some(79), "{error}");
    // termination.rs documents that the pinned runner escapes the id with
    // `escape_debug`. If a pin bump changes that, update those docs and
    // check that `leaf_exit_code` still matches the new form.
    assert!(
        error.starts_with("Step 'a\\'b' failed: "),
        "the pinned runner no longer escapes step ids as termination.rs says: {error:?}"
    );
}

#[test]
fn a_real_bash_exit_from_the_pinned_adapter_decodes_its_status() {
    for (id, status) in [
        ("step-02-merge-ready-loop", 79),
        ("a'b", 79),
        ("step-04-run", 1),
    ] {
        let script = format!(
            "printf 'ERROR: exit 79 terminal policy refusal in the merge-ready loop\\n' >&2; exit {status}"
        );
        let error = real_bash_failure(id, &script);
        assert_eq!(decode(id, &error), Some(status), "{error}");
    }
}

/// The adapter's real text inside the copied sub-recipe header: the path a
/// refused `autodrive-merge-loop` takes up to `auto-drive-to-merge`.
#[test]
fn a_real_bash_refusal_inside_a_sub_recipe_decodes_as_79() {
    let inner = "step-02-merge-ready-loop";
    let leaf = real_bash_failure(
        inner,
        "printf 'ERROR: terminal policy refusal\\nsecond line\\n' >&2; exit 79",
    );
    let error = sub_recipe(
        "autodrive-merge-loop",
        "autodrive-merge-loop",
        &[(inner, leaf)],
        &["step-01-merge-loop-preflight"],
    );
    assert_eq!(decode("autodrive-merge-loop", &error), Some(79), "{error}");
}
