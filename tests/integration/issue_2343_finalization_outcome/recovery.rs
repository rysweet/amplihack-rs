//! What happens when nothing decides the terminal state (#2343, #2809).
//!
//! Since the decision is `assess-finalization`'s, a run that reaches
//! `validate-agentic-finalization` with no usable decision has a gap in
//! finalization, not a verdict about the run. The step is left failing, which
//! is what sends it to its own recovery agent. That agent does not repair:
//! the step is wired `--no-repair`, because every input was recorded before
//! it ran and nothing the agent may do would change them. It stops with the
//! reason (docs/reference/step-failure-recovery.md). Re-running the decision
//! is `reassess-finalization`'s job, upstream of this step.
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

// #2819: the two gaps are not the same gap, so they are not reported in
// the same words. An answer naming a state finalization does not publish is
// quoted back. An absent one is reported as the assessment having delivered
// nothing -- after `reassess-finalization` upstream also produced nothing --
// which is what the operator acts on: the model call is what to look at, not
// the run's artifacts.
#[test]
fn an_absent_answer_and_an_unpublishable_one_are_reported_as_different_gaps() {
    let fx = Fixture::new();
    fx.with_remote();
    fx.push();
    fx.gh_pr_at(&fx.head(), green_rollup_for_tests());

    let absent = fx.validate(&as_refs(&finished_env(&fx.repo_path())));
    let unpublishable = fx.validate(&as_refs(&decided(&fx, "TOTALLY_FINE", "7")));

    assert_no_decision(&absent, "nothing was delivered");
    assert_no_decision(&unpublishable, "what was delivered is not published");
    assert!(
        absent.field("terminal_reason").contains("re-run"),
        "an absent answer must say both attempts came back empty: {}",
        absent.field("terminal_reason")
    );
    assert!(
        !absent.field("terminal_reason").contains("re-run")
            || !unpublishable.field("terminal_reason").contains("re-run"),
        "a delivered answer must not be reported as an undelivered one"
    );
    assert!(
        unpublishable
            .field("terminal_reason")
            .contains("TOTALLY_FINE"),
        "an unpublishable answer must be quoted back: {}",
        unpublishable.field("terminal_reason")
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
