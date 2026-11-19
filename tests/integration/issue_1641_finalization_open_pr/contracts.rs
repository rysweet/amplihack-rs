//! Static contracts: the typed-evidence-only rule (#969) and the field
//! allowlist (#1538).
//!
//! #2819 R1: the three cases #2343 added here are gone -- the byte offsets of
//! `source` lines inside the dispatcher, the grep of the facts reader for the
//! helper names it reuses, and the read of `cycle2_module_budget.rs`'s own
//! source for the files it lists. `docs/howto/keep-tests-passing-under-host-load.md`
//! forbids asserting on how code is written: such a case fails when the
//! wording moves and passes when the behaviour moves. What they stood for is
//! run instead -- every case that calls `Fixture::validate` executes the real
//! dispatcher, so a helper it fails to load, or loads too late to use, fails
//! there; and the 300-line budget is enforced by `cycle2_module_budget.rs`
//! measuring the files themselves, which needs no second test reading its
//! list.

use super::fixture::*;

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
// The classifier stays deleted because nothing sources it: the dispatcher
// runs under `set -e`, so a `.` of a file that is not there ends it, and
// every case that validates would fail. This asserts only that the file is
// absent, which no run can show.
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
//
// The 300-line budget for the finalization helpers was asserted here too, by
// reading `cycle2_module_budget.rs`'s own source for the paths it lists. That
// test measures the files; this one only read its text, so it could pass with
// the budget unenforced and fail with it enforced under a different spelling.
