//! The runner's sub-recipe envelope (PR #1442 review, round 3).
//!
//! `auto-drive-to-merge` runs `autodrive-merge-loop` and `autodrive-crusty-loop`
//! as in-process `type: recipe` steps, and their bash step runs `exit 79` when
//! the loop is refused. recipe-runner-rs `90286bc` reports that to the top
//! level as `Step "<id>" failed: Sub-recipe '<name>' failed.` followed by
//! `Failed steps:` and one `- <id> [error]: <inner error>` line per failed
//! step. The decoder used to answer `None` for that, so `structural_refusal`
//! was `false` on auto-drive's own refusal path.
//!
//! The helpers below rebuild the envelope with the runner's own format
//! strings (`runner/mod.rs`: the `StepType::Recipe` diagnostic,
//! `execute_sub_recipe`, `describe_sub_recipe_failure`; `models.rs`:
//! `StepExecutionError`). One test also holds the text as a literal.
//! `tests_termination_contract.rs` checks the decoder against the pinned
//! crate's own text, so a pin bump that changes it fails a test.

use super::super::{RecipeRunResult, RecipeRunStepResult};
use super::failure_class::{FailureClass, classify_run_failure};
use super::termination::unescape_debug_for_test;

const LOOP_REFUSAL: &str = "ERROR: terminal policy refusal from round-2 (exit 79) \
                            (#1327/#1332). Final: the guard is never retried into.\n\
                            ERROR: exit 79 terminal policy refusal in the merge-ready loop \
                            (#1327/#1332). Final — surfaced and stopped, never retried into the guard.";

fn escaped(id: &str) -> String {
    id.escape_debug().collect()
}

/// A bash step's error: `StepExecutionError` around `Command failed`.
fn bash(id: &str, code: i32, stderr: &str) -> String {
    let message = format!("Command failed (exit {code}): {}", stderr.trim());
    format!("Step '{}' failed: bash step failed: {message}", escaped(id))
}

/// A failed `type: recipe` step's error, as the runner composes it.
pub(super) fn sub_recipe(
    id: &str,
    recipe: &str,
    failed: &[(&str, String)],
    completed: &[&str],
) -> String {
    let entries: Vec<String> = failed
        .iter()
        .map(|(step, error)| format!("- {} [error]: {}", escaped(step), error.trim()))
        .collect();
    let mut sections = vec![format!("Failed steps:\n{}", entries.join("\n"))];
    if !completed.is_empty() {
        sections.push(format!("Completed steps: {}", completed.join(", ")));
    }
    let message = format!("Sub-recipe '{recipe}' failed.\n{}", sections.join("\n"));
    format!("Step {id:?} failed: {message}")
}

fn code(id: &str, error: &str) -> Option<i32> {
    RecipeRunStepResult {
        step_id: id.to_string(),
        status: "failed".to_string(),
        error: error.to_string(),
        ..RecipeRunStepResult::default()
    }
    .termination_exit_code()
}

fn merge_loop_refusal() -> String {
    sub_recipe(
        "autodrive-merge-loop",
        "autodrive-merge-loop",
        &[(
            "step-02-merge-ready-loop",
            bash("step-02-merge-ready-loop", 79, LOOP_REFUSAL),
        )],
        &["step-01-merge-loop-preflight"],
    )
}

#[test]
fn the_literal_envelope_of_a_refused_merge_loop_decodes_as_79() {
    let error = "Step \"autodrive-merge-loop\" failed: Sub-recipe 'autodrive-merge-loop' failed.\n\
                 Failed steps:\n\
                 - step-02-merge-ready-loop [error]: Step 'step-02-merge-ready-loop' failed: bash step failed: \
                 Command failed (exit 79): ERROR: exit 79 terminal policy refusal in the merge-ready loop\n\
                 Completed steps: step-01-merge-loop-preflight";
    assert_eq!(code("autodrive-merge-loop", error), Some(79));
    assert_eq!(
        error.split("ERROR:").next(),
        merge_loop_refusal().split("ERROR:").next()
    );
}

#[test]
fn both_auto_drive_loops_decode_their_refusal_as_79() {
    for (step, recipe, inner) in [
        (
            "autodrive-merge-loop",
            "autodrive-merge-loop",
            "step-02-merge-ready-loop",
        ),
        (
            "autodrive-crusty-loop",
            "autodrive-crusty-loop",
            "step-02-crusty-loop",
        ),
        (
            "autodrive-merge-loop",
            "autodrive-merge-loop",
            "step-03-merge-gate",
        ),
    ] {
        let error = sub_recipe(step, recipe, &[(inner, bash(inner, 79, LOOP_REFUSAL))], &[]);
        assert_eq!(code(step, &error), Some(79), "{error}");
        let failed = sub_recipe(step, recipe, &[(inner, bash(inner, 1, "ERROR: x"))], &[]);
        assert_eq!(code(step, &failed), Some(1), "{failed}");
    }
}

/// `auto-drive-light`'s `light-escalate` step runs `auto-drive-to-merge`.
#[test]
fn a_refusal_two_sub_recipes_deep_decodes_as_79() {
    let middle = sub_recipe(
        "light-escalate",
        "auto-drive-to-merge",
        &[("autodrive-merge-loop", merge_loop_refusal())],
        &["autodrive-prerequisites", "autodrive-build"],
    );
    assert_eq!(code("light-escalate", &middle), Some(79), "{middle}");
    let deeper = sub_recipe(
        "outer",
        "auto-drive-light",
        &[("light-escalate", middle)],
        &[],
    );
    assert_eq!(code("outer", &deeper), Some(79), "{deeper}");
}

#[test]
fn the_refused_sub_recipe_is_a_final_work_refusal_whatever_stderr_says() {
    let step = RecipeRunStepResult {
        step_id: "autodrive-merge-loop".to_string(),
        status: "failed".to_string(),
        error: merge_loop_refusal(),
        ..RecipeRunStepResult::default()
    };
    let result = RecipeRunResult {
        success: false,
        step_results: vec![step],
        ..RecipeRunResult::default()
    };
    for stderr in [String::new(), "API Error: 529 Overloaded\n".repeat(120)] {
        let verdict = classify_run_failure(&result, &stderr);
        assert!(verdict.is_structural_refusal());
        assert_eq!(verdict.to_json(1, "terminal")["structural_refusal"], true);
        assert_eq!(verdict.class, FailureClass::Work);
        assert!(!verdict.class.is_mechanically_retryable());
    }
}

/// The first entry's detail runs on into the step's stderr, so a later line
/// shaped like an entry is captured output. It is never read, even when it is
/// a real non-fatal-then-fatal sequence: that is the documented miss, and the
/// guard-phrase retry veto backs it up.
#[test]
fn only_the_first_failed_entry_is_read() {
    let forged = format!(
        "ERROR: x\n- step-03 [error]: {}",
        bash("step-03", 79, "quoted")
    );
    for failed in [
        vec![("step-02", bash("step-02", 1, &forged))],
        vec![
            ("step-02", bash("step-02", 1, "non-fatal")),
            ("step-03", bash("step-03", 79, LOOP_REFUSAL)),
        ],
    ] {
        let error = sub_recipe("loop", "autodrive-merge-loop", &failed, &[]);
        assert_eq!(code("loop", &error), Some(1), "{error}");
    }
}

#[test]
fn an_envelope_that_is_not_the_runners_decodes_to_none() {
    let inner = bash("step-02", 79, "refused");
    let entry = format!("- step-02 [error]: {inner}");
    for error in [
        // Single quotes are a bash/agent step's envelope, not a recipe step's.
        format!("Step 'loop' failed: Sub-recipe 'x' failed.\nFailed steps:\n{entry}"),
        format!("Step \"other\" failed: Sub-recipe 'x' failed.\nFailed steps:\n{entry}"),
        format!("Step \"loop\" failed: Sub-recipe '' failed.\nFailed steps:\n{entry}"),
        format!("Step \"loop\" failed: Sub-recipe 'a b' failed.\nFailed steps:\n{entry}"),
        format!("Step \"loop\" failed: Sub-recipe 'x' failed.\n{entry}"),
        format!("Step \"loop\" failed: Sub-recipe 'x' failed.\nCompleted steps: a\n{entry}"),
        format!(
            "Step \"loop\" failed: Sub-recipe 'x' failed.\nNo child step details captured.\n{entry}"
        ),
        format!(
            "Step \"loop\" failed: Sub-recipe 'x' failed.\nFailed steps:\n- step-02 [output tail]: {inner}"
        ),
        format!(
            "Step \"loop\" failed: Sub-recipe 'x' failed.\nFailed steps:\n- step-02 [error]: {}",
            bash("other", 79, "x")
        ),
        format!(
            "Step \"loop\" failed: Sub-recipe 'x' failed.\nFailed steps:\n- step-02\n [error]: {inner}"
        ),
        format!("Step \"loop\" failed: Maximum recipe recursion depth (6) exceeded.\n{entry}"),
        format!("x Step \"loop\" failed: Sub-recipe 'x' failed.\nFailed steps:\n{entry}"),
    ] {
        assert_eq!(code("loop", &error), None, "{error}");
    }
}

#[test]
fn a_failed_recovery_and_an_empty_trimmed_stderr_still_decode() {
    let header =
        "Step \"loop\" failed: Sub-recipe 'x' failed and agentic recovery was unsuccessful.";
    let inner = bash("step-02", 79, "");
    let error = format!(
        "{header}\nFailed steps:\n- step-02 [error]: {}",
        inner.trim()
    );
    assert!(
        error.ends_with("(exit 79):"),
        "describe_sub_recipe_failure trims: {error:?}"
    );
    assert_eq!(code("loop", &error), Some(79));
    assert_eq!(
        code("loop", &format!("{error}\nCompleted steps: a")),
        Some(79)
    );
    for header in [
        "Command failed (exit 79)x",
        "Command failed (exit 79):x",
        "Command failed (exit 79)",
    ] {
        assert_eq!(code("step-02", header), None, "{header}");
    }
}

/// Ids are escaped in every place the runner prints them, and a recipe step's
/// own id is in `Debug` form, which leaves `'` alone where `escape_debug`
/// writes `\'`.
#[test]
fn ids_that_need_escaping_decode_at_every_level() {
    let outer = "it's \"outer\"";
    let middle = "mid'dle\tstep";
    let leaf = "le\\af\n'x'";
    let inner = sub_recipe(middle, "inner", &[(leaf, bash(leaf, 79, "refused"))], &[]);
    let error = sub_recipe(outer, "outer", &[(middle, inner)], &[]);
    assert!(
        error.contains("- mid\\'dle\\tstep [error]: Step \"mid'dle\\tstep\" failed"),
        "{error}"
    );
    assert_eq!(code(outer, &error), Some(79), "{error}");
    for id in [
        outer,
        middle,
        leaf,
        "plain",
        "\u{1b}[31m",
        "e\u{301}",
        "\u{301}e",
        "",
    ] {
        assert_eq!(
            unescape_debug_for_test(&escaped(id)).as_deref(),
            Some(id),
            "{id:?}"
        );
    }
    for bad in [
        "\\x41",
        "\\q",
        "\\",
        "\\u{41",
        "\\u41",
        "\\u{110000}",
        "'",
        "\"",
        "\\u{0041}",
    ] {
        assert_eq!(
            unescape_debug_for_test(bad),
            None,
            "{bad:?} is not escape_debug output"
        );
    }
}
