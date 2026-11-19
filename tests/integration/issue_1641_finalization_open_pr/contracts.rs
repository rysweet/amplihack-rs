//! Static contracts: the typed-evidence-only rule (#969) and the field
//! allowlist (#1538).
//!
//! These two cases are deliberately all that is read rather than run.
//! `docs/howto/keep-tests-passing-under-host-load.md` forbids asserting on how
//! code is written -- such a case fails when the wording moves and passes when
//! the behaviour moves -- and every other contract this directory covers is
//! executed instead, by the cases that call `Fixture::validate`.

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

#[test]
fn complete_helper_lists_existing_open_pr_in_vocabulary() {
    let complete = tool_text("workflow_finalization_complete.sh");
    assert!(
        complete.contains("\"FOLLOWUP_CREATED\", \"EXISTING_OPEN_PR\""),
        "EXISTING_OPEN_PR must follow FOLLOWUP_CREATED in terminal_vocabulary"
    );
}
