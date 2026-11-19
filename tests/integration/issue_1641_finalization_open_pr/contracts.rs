//! Static contracts: wiring, the typed-evidence-only rule (#969), the field
//! allowlist (#1538), the #2343 separation of collecting facts from deciding
//! on them, and the line budget.

use super::fixture::*;

#[test]
fn dispatcher_sources_helpers_in_dependency_order() {
    let dispatcher = tool_text("workflow_agentic_finalization.sh");
    let find = |needle: &str| {
        dispatcher
            .find(needle)
            .unwrap_or_else(|| panic!("dispatcher must source {needle}"))
    };
    let retry = find(". \"$here/workflow_gh_retry.sh\"");
    let signal = find(". \"$here/workflow_finalization_pr_signal.sh\"");
    let outcome = find(". \"$here/workflow_finalization_outcome.sh\"");
    let result = find(". \"$here/workflow_finalization_result.sh\"");
    let handoff = find(". \"$here/workflow_finalization_handoff.sh\"");
    let validate = find(". \"$here/workflow_finalization_validate.sh\"");
    assert!(
        retry < validate && signal < validate,
        "helpers must load before the validator"
    );
    assert!(
        signal < outcome,
        "the facts reader reuses the pr-signal helpers, so it loads after them"
    );
    assert!(
        result < handoff && result < validate,
        "the hand-off brick and the validator both emit through the result brick, \
         so it must load before both"
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

    // #2343: the facts reader replaced the open-PR check. It reuses the shared
    // rules for "what does this check rollup mean" and "does this commit
    // descend from the local head" rather than restating them, and neither it
    // nor the validator reads prose or raw pre-commit reports.
    let outcome = tool_text("workflow_finalization_outcome.sh");
    for required in [
        "_finalization_pr_rollup_class",
        "_finalization_pr_head_descends",
    ] {
        assert!(
            outcome.contains(required),
            "the facts reader must reuse `{required}` rather than restate its rule"
        );
    }
    let context = tool_text("workflow_finalization_context.sh");
    assert!(
        !context.contains("statusCheckRollup") && !context.contains("precommit_results"),
        "the workflow_finalization_field allowlist must not be widened"
    );
}

// The two cases that sat here grepped the reader and the validator for
// forbidden words, and compared the byte offsets of `gather_outcome_facts` and
// `finalization_assessment terminal_state` inside the validator. Both are now
// run rather than read: `issue_2343_finalization_outcome::facts` drives the
// reader over every outcome a classifier would have split on and asserts it
// names none, and `issue_2343_finalization_outcome::terminal_states` asserts
// that a blocker beats a decision and that a decision is never applied to
// facts that were not gathered -- which is what the ordering was for.
//
// The deleted classifier stays deleted because nothing sources it; the
// dispatcher case above is what would fail if anything did.
#[test]
fn nothing_sources_the_deleted_outcome_classifier() {
    assert!(
        !workspace_root()
            .join("amplifier-bundle/tools/workflow_finalization_outcome_classify.sh")
            .exists(),
        "workflow_finalization_outcome_classify.sh must stay deleted (#2343)"
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

// The vocabulary the validator publishes and the one workflow-complete
// reports must agree. That used to be asserted by parsing the predicate bodies
// out of the shell source; it is asserted by running both steps over every
// state in `issue_2343_finalization_outcome::wiring`.

#[test]
fn module_budget_guards_the_finalization_helpers() {
    let budget = std::fs::read_to_string(
        workspace_root().join("crates/amplihack-cli/tests/cycle2_module_budget.rs"),
    )
    .expect("read cycle2 budget");
    for helper in [
        "workflow_finalization_pr_signal.sh",
        "workflow_finalization_outcome.sh",
        "workflow_finalization_result.sh",
        "workflow_finalization_validate.sh",
    ] {
        assert!(
            budget.contains(&format!("../../amplifier-bundle/tools/{helper}")),
            "{helper} must be under the cycle-2 300-line guard"
        );
    }
    assert!(
        !budget.contains("workflow_finalization_outcome_classify.sh"),
        "the budget must not name the deleted classifier"
    );
}
