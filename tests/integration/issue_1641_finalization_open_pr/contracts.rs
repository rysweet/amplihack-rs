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

    // The open-PR check lives in its own brick (#1714) and the validator
    // calls it; neither reads prose or raw pre-commit reports.
    let open_pr = tool_text("workflow_finalization_open_pr.sh");
    assert!(
        open_pr.contains("finalization_open_pr_signal"),
        "the open-PR check must use the pr signal helper"
    );
    assert!(
        !open_pr.contains("agentic_finalizer_narrative") && !open_pr.contains("precommit_results"),
        "the open-PR check must not read prose or raw precommit reports"
    );
    let validate = tool_text("workflow_finalization_validate.sh");
    assert!(
        validate.contains("confirm_open_pr"),
        "validator must use the open-PR check"
    );
    assert!(
        validate.contains("EXISTING_OPEN_PR"),
        "validator must classify EXISTING_OPEN_PR"
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

#[test]
fn open_pr_path_runs_after_no_op_and_before_the_default_failure() {
    let validate = tool_text("workflow_finalization_validate.sh");
    let no_op = validate
        .find("classify_success \"ALLOW_NO_OP\"")
        .expect("ALLOW_NO_OP success path");
    let open_pr = validate
        .find("classify_success \"EXISTING_OPEN_PR\"")
        .expect("EXISTING_OPEN_PR success path");
    let default = validate
        .rfind("finalization_fail_or_handoff_open_pr \"FAILED_IMPLEMENTATION\"")
        .expect("default FAILED_IMPLEMENTATION (#1714 routes it through the hand-off brick)");
    let hollow = validate
        .find("\"HOLLOW_SUCCESS\"")
        .expect("hollow-success blocker");
    assert!(
        hollow < no_op && no_op < open_pr && open_pr < default,
        "EXISTING_OPEN_PR must sit after every blocker and ALLOW_NO_OP, before the default"
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
