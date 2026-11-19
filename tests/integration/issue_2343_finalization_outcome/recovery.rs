//! What happens when nothing decides the terminal state (#2343, #2809).
//!
//! Since the decision is `assess-finalization`'s, a run that reaches
//! `validate-agentic-finalization` with no usable decision has a gap in
//! finalization, not a verdict about the run. The step is left failing, which
//! is what sends it to its own recovery agent: that agent repairs and
//! validates again, or stops with the reason
//! (docs/reference/step-failure-recovery.md).

use super::fixture_ext::*;
use crate::issue_1641_finalization_open_pr::builders::{as_refs, assessment, resumed_env};
use crate::issue_1641_finalization_open_pr::fixture::*;

fn recipe(name: &str) -> String {
    let path = workspace_root().join(format!("amplifier-bundle/recipes/{name}.yaml"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The `<id>` step of `<recipe>.yaml`, from its `- id:` line to the line
/// before the next step.
fn step(recipe_name: &str, id: &str) -> String {
    let text = recipe(recipe_name);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(&format!("- id: \"{id}\"")))
        .unwrap_or_else(|| panic!("{recipe_name}.yaml must define a {id} step"));
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with("- id:"))
        .map_or(lines.len(), |offset| start + 1 + offset);
    lines[start..end].join("\n")
}

fn assert_no_decision(outcome: &Outcome, why: &str) {
    assert_eq!(
        outcome.output.status.code(),
        Some(1),
        "{why}: the step must be left failing\n{}",
        outcome.describe()
    );
    assert_eq!(
        outcome.field("terminal_state"),
        "FAILED_MISSING_TERMINAL_EVIDENCE",
        "{why}\n{}",
        outcome.describe()
    );
    assert_eq!(outcome.field("terminal_success"), "false", "{why}");
    assert!(
        outcome
            .field("terminal_reason")
            .contains("no terminal state"),
        "{why}: the reason must say the decision is missing: {}",
        outcome.field("terminal_reason")
    );
}

// The deputy's second outcome case: a run with nothing pushed is not
// classified a failure by the validator itself. It ends with no decision, and
// the reason names the real gap, so recovery has something to act on.
#[test]
fn a_run_with_nothing_pushed_and_no_assessment_ends_with_no_decision() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.gh_no_pr();
    let local = fx.head();

    let outcome = fx.validate(&as_refs(&resumed_env(&fx.repo_path())));

    assert_no_decision(&outcome, "nothing pushed, nothing decided");
    let evidence = outcome.field("evidence_used");
    for fact in [BRANCH, local.as_str(), "assessment_state=none"] {
        assert!(
            evidence.contains(fact),
            "the gathered facts must reach the record (`{fact}`): {evidence}"
        );
    }
    assert_ne!(
        outcome.field("terminal_state"),
        "FAILED_IMPLEMENTATION",
        "the validator must not reach a verdict of its own"
    );
}

// The same holds for a finished run: no decision is no decision, whatever the
// facts look like. Finalization never fills one in from the facts.
#[test]
fn a_finished_run_with_no_assessment_also_ends_with_no_decision() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup_for_tests());

    let outcome = fx.validate(&as_refs(&finished_env(&fx.repo_path())));

    assert_no_decision(&outcome, "a green pull request is still not a decision");
    assert!(
        outcome
            .field("evidence_used")
            .contains("outcome.checks=green"),
        "the facts were gathered: {}",
        outcome.field("evidence_used")
    );
}

#[test]
fn a_state_outside_the_published_vocabulary_is_no_decision_and_is_quoted_back() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup_for_tests());
    let mut envs = finished_env(&fx.repo_path());
    envs.push(assessment("TOTALLY_FINE", "none", "7"));

    let outcome = fx.validate(&as_refs(&envs));

    assert_no_decision(&outcome, "an unpublished state names no terminal state");
    assert!(
        outcome.field("terminal_reason").contains("TOTALLY_FINE"),
        "the reason must quote what was returned: {}",
        outcome.field("terminal_reason")
    );
}

// The wiring that makes the paragraph above true. Recovery on this step must
// offer repair: `--guard` and `--no-repair` both withhold `retry_after_repair`
// (docs/reference/step-failure-recovery.md#the-four-choices), which would
// leave an agent that can only stop.
#[test]
fn the_validator_step_goes_to_a_recovery_agent_that_may_repair_and_validate_again() {
    let text = step("workflow-finalize", "validate-agentic-finalization");
    assert!(
        text.contains("step-recovery run --step-id validate-agentic-finalization"),
        "the validator step must run under step recovery:\n{text}"
    );
    for withholding in ["--guard", "--no-repair"] {
        assert!(
            !text.contains(withholding),
            "`{withholding}` withholds retry_after_repair, so recovery could only stop:\n{text}"
        );
    }
    assert!(
        text.contains("--stop-on-handoff"),
        "a recorded handoff must end the run rather than reach the steps that parse this output:\n{text}"
    );
}

// The assessment runs on every outcome. It carried a condition that skipped it
// for the verdicts the old bash classifier exited on; both the verdicts and
// the classifier are gone, and with them the two-file set that had to agree.
#[test]
fn the_assessment_runs_on_every_outcome_and_never_ends_the_run() {
    let text = step("workflow-finalize", "assess-finalization");
    assert!(
        !text.contains("condition:"),
        "assess-finalization decides every run's terminal state, so it is never skipped:\n{text}"
    );
    for name in ["workflow-finalize", "workflow-finalization-assessment"] {
        assert!(
            step(name, "assess-finalization").contains("continue_on_error: true"),
            "{name}.yaml's assess-finalization step must carry continue_on_error: true"
        );
    }
}

// The validator reports a state; it must not contain one of its own.
#[test]
fn the_validator_holds_no_outcome_classification_of_its_own() {
    let text = tool_text("workflow_finalization_validate.sh");
    for forbidden in [
        "EXISTING_OPEN_PR",
        "BLOCKED_CI\"",
        "classify_confirmed_outcome",
        "pr_head_relation",
        "branch_pushed",
    ] {
        assert!(
            !text.contains(forbidden),
            "the validator must not classify the run's outcome (`{forbidden}`)"
        );
    }
    assert!(
        text.contains("finalization_assessment terminal_state"),
        "the validator must take the terminal state from the assessment"
    );
}

// The vocabulary the validator accepts is the one workflow-complete publishes.
#[test]
fn every_state_the_validator_accepts_is_in_the_published_vocabulary() {
    let result = tool_text("workflow_finalization_result.sh");
    let predicates = result
        .split_once("_finalization_state_allowed()")
        .expect("the result brick defines the vocabulary predicates")
        .1;
    let vocabulary = tool_text("workflow_finalization_complete.sh");
    let mut seen = 0;
    for token in predicates
        .split(|c: char| !(c.is_ascii_uppercase() || c == '_'))
        .filter(|t| t.len() >= 6 && t.contains('_'))
    {
        seen += 1;
        assert!(
            vocabulary.contains(&format!("\"{token}\"")),
            "`{token}` is not in terminal_vocabulary in workflow_finalization_complete.sh"
        );
    }
    assert!(
        seen >= 10,
        "the predicates must name the vocabulary, saw {seen}"
    );
}
