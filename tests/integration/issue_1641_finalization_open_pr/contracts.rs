//! Static contracts: wiring, the typed-evidence-only rule (#969), the field
//! allowlist (#1538) and the line budget.

use super::fixture::*;

#[test]
fn dispatcher_sources_gh_retry_and_pr_signal_before_the_validator() {
    let dispatcher = tool_text("workflow_agentic_finalization.sh");
    let retry = dispatcher
        .find(". \"$here/workflow_gh_retry.sh\"")
        .expect("dispatcher must source workflow_gh_retry.sh");
    let signal = dispatcher
        .find(". \"$here/workflow_finalization_pr_signal.sh\"")
        .expect("dispatcher must source workflow_finalization_pr_signal.sh");
    let validate = dispatcher
        .find(". \"$here/workflow_finalization_validate.sh\"")
        .expect("dispatcher sources the validator");
    assert!(
        retry < validate && signal < validate,
        "helpers must load before the validator"
    );
}

#[test]
fn open_pr_path_uses_only_typed_evidence_and_bounded_read_only_gh() {
    let helper = tool_text("workflow_finalization_pr_signal.sh");
    assert!(helper.contains("finalization_open_pr_signal()"));
    for required in [
        "_gh_retry_core",
        "GH_RETRY_MAX_RL_WINDOWS=0",
        "GH_RETRY_MAX_TRANSIENT=1",
        "mktemp -d",
        "--repo",
    ] {
        assert!(
            helper.contains(required),
            "pr signal helper must use `{required}`"
        );
    }
    for forbidden in [
        "agentic_finalizer_narrative",
        "AGENTIC_FINALIZER",
        "eval ",
        "sh -c",
        "gh auth",
        "GH_TOKEN",
        "pr create",
        "pr edit",
        "pr merge",
        "pr comment",
        "pr ready",
    ] {
        assert!(
            !helper.contains(forbidden),
            "pr signal helper must not contain `{forbidden}`"
        );
    }
    assert!(
        helper.lines().count() <= 300,
        "workflow_finalization_pr_signal.sh must stay within the 300-line budget"
    );

    // #2343: the outcome reader replaced the open-PR check. It reuses the pr
    // signal helper's rules for "is this PR this run's PR" rather than
    // restating them, and neither it nor the validator reads prose or raw
    // pre-commit reports.
    let outcome = tool_text("workflow_finalization_outcome.sh");
    for required in ["_finalization_pr_checks", "_finalization_pr_head_descends"] {
        assert!(
            outcome.contains(required),
            "the outcome reader must reuse `{required}` rather than restate its rule"
        );
    }
    let classify = tool_text("workflow_finalization_outcome_classify.sh");
    for (name, text) in [("reader", &outcome), ("classifier", &classify)] {
        assert!(
            !text.contains("agentic_finalizer_narrative") && !text.contains("precommit_results"),
            "the outcome {name} must not read prose or raw precommit reports"
        );
    }
    assert!(
        classify.contains("EXISTING_OPEN_PR"),
        "the outcome classifier must classify EXISTING_OPEN_PR"
    );
    assert!(
        !outcome.contains("EXISTING_OPEN_PR") && !outcome.contains("terminal_state"),
        "the outcome reader reports facts; it must name no terminal state"
    );
    let validate = tool_text("workflow_finalization_validate.sh");
    assert!(
        validate.contains("classify_confirmed_outcome"),
        "validator must classify from the run's real outcome"
    );
    assert!(
        !validate.contains("agentic_finalizer_narrative")
            && !validate.contains("precommit_results"),
        "validator must not read prose or raw precommit reports"
    );

    let context = tool_text("workflow_finalization_context.sh");
    assert!(
        !context.contains("statusCheckRollup") && !context.contains("precommit_results"),
        "the workflow_finalization_field allowlist must not be widened"
    );
}

// #2343 inverted one pair here: the run's real outcome is read BEFORE the
// typed completion booleans, so a published pull request decides the result
// and the booleans are only a fallback for a run with no such pull request to
// read. Every blocker still precedes both, and the gap-bearing failure is
// still last.
#[test]
fn the_outcome_read_runs_after_every_blocker_and_before_the_bookkeeping_fallback() {
    let validate = tool_text("workflow_finalization_validate.sh");
    let outcome = validate
        .find("classify_confirmed_outcome \"\"")
        .expect("the outcome classification");
    let no_op = validate
        .find("classify_success \"ALLOW_NO_OP\"")
        .expect("ALLOW_NO_OP success path");
    let verified = validate
        .find("classify_success \"IMPLEMENTED_VERIFIED\"")
        .expect("IMPLEMENTED_VERIFIED success path");
    let default = validate
        .rfind("finalization_fail_or_handoff_open_pr \"FAILED_IMPLEMENTATION\"")
        .expect("default FAILED_IMPLEMENTATION (#1714 routes it through the hand-off brick)");
    let hollow = validate
        .find("\"HOLLOW_SUCCESS\"")
        .expect("hollow-success blocker");
    let dirty = validate
        .find("\"FAILED_DIRTY_WORKTREE\"")
        .expect("dirty-worktree blocker");
    assert!(
        dirty < hollow && hollow < outcome,
        "every blocker precedes the outcome read"
    );
    assert!(
        outcome < verified && verified < no_op && no_op < default,
        "the outcome read precedes the bookkeeping fallback, which precedes the default"
    );
}

#[test]
fn complete_helper_lists_existing_open_pr_in_vocabulary() {
    let complete = tool_text("workflow_finalization_complete.sh");
    assert!(
        complete.contains("\"FOLLOWUP_CREATED\", \"EXISTING_OPEN_PR\""),
        "EXISTING_OPEN_PR must follow FOLLOWUP_CREATED in terminal_vocabulary"
    );
}

#[test]
fn module_budget_guards_the_new_helper() {
    let budget = std::fs::read_to_string(
        workspace_root().join("crates/amplihack-cli/tests/cycle2_module_budget.rs"),
    )
    .expect("read cycle2 budget");
    assert!(
        budget.contains("../../amplifier-bundle/tools/workflow_finalization_pr_signal.sh"),
        "the new helper must be under the cycle-2 300-line guard"
    );
}
