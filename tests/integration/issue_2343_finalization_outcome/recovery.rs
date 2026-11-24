//! What happens when nothing decides the terminal state (#2343, #2809).
//!
//! Since the decision is `assess-finalization`'s, a run that reaches
//! `validate-agentic-finalization` with no usable decision has a gap in
//! finalization, not a verdict about the run. The step is left failing, which
//! is what sends it to its own recovery agent: that agent repairs and
//! validates again, or stops with the reason
//! (docs/reference/step-failure-recovery.md).
//!
//! These cases drive the step. The wiring that carries their exit to a
//! recovery agent is asserted by running that wiring, in `wiring.rs`.

use super::fixture_ext::*;
use crate::issue_1641_finalization_open_pr::builders::{as_refs, assessment, resumed_env};
use crate::issue_1641_finalization_open_pr::fixture::*;

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
    // With no assessment at all there is no action either: `none`, not a value
    // the validator could not read.
    for fact in [
        BRANCH,
        local.as_str(),
        "assessment_state=none",
        "assessment_action=none,",
    ] {
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

// The four cases that used to sit here read recipe text and shell text: the
// step's `--guard`/`--essential` flags, the assessment step's `condition:`,
// the words the validator contains, and the byte order of the vocabulary
// predicates. They are in `wiring.rs` now, as cases that run the shipped steps
// and read what they did.
