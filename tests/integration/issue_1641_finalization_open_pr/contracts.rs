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
    let validate = tool_text("workflow_finalization_validate.sh");
    for (name, text) in [("reader", &outcome), ("validator", &validate)] {
        assert!(
            !text.contains("agentic_finalizer_narrative") && !text.contains("precommit_results"),
            "the outcome {name} must not read prose or raw precommit reports"
        );
    }

    let context = tool_text("workflow_finalization_context.sh");
    assert!(
        !context.contains("statusCheckRollup") && !context.contains("precommit_results"),
        "the workflow_finalization_field allowlist must not be widened"
    );
}

// #2343: collecting the facts and deciding what they mean are different files.
// The reader names no terminal state at all, and the validator reaches none of
// its own: it reports the one assess-finalization decided.
#[test]
fn the_facts_reader_names_no_terminal_state_and_the_validator_decides_none() {
    let outcome = tool_text("workflow_finalization_outcome.sh");
    for forbidden in [
        "EXISTING_OPEN_PR",
        "BLOCKED_CI",
        "terminal_state",
        "verdict=",
    ] {
        assert!(
            !outcome.contains(forbidden),
            "the facts reader reports facts; it must not contain `{forbidden}`"
        );
    }

    let validate = tool_text("workflow_finalization_validate.sh");
    for forbidden in [
        "EXISTING_OPEN_PR",
        "classify_confirmed_outcome",
        "read_live_outcome",
    ] {
        assert!(
            !validate.contains(forbidden),
            "the validator must classify no outcome of its own; it must not contain `{forbidden}`"
        );
    }
    assert!(
        validate.contains("finalization_assessment terminal_state"),
        "the validator must take the terminal state from the assessment"
    );

    // The classifier the deputy asked to be removed is gone, and nothing
    // sources it.
    assert!(
        !workspace_root()
            .join("amplifier-bundle/tools/workflow_finalization_outcome_classify.sh")
            .exists(),
        "workflow_finalization_outcome_classify.sh must be deleted"
    );
    assert!(
        !tool_text("workflow_agentic_finalization.sh").contains("outcome_classify"),
        "nothing may source the deleted classifier"
    );
}

// #2343 ordering: the blockers about the local checkout run first, because no
// published outcome can answer them; the facts are gathered next; the decision
// is read last, so it can never be applied to facts that were not gathered.
#[test]
fn blockers_precede_the_facts_which_precede_the_decision() {
    let validate = tool_text("workflow_finalization_validate.sh");
    let body = validate
        .split_once("validate_finalization() {")
        .expect("validate_finalization must be defined")
        .1;
    let dirty = body
        .find("\"FAILED_DIRTY_WORKTREE\"")
        .expect("dirty-worktree blocker");
    let prior = body
        .find("deterministic terminal probe reported")
        .expect("prior terminal probe blocker");
    let facts = body
        .find("\n  gather_outcome_facts")
        .expect("the facts are gathered");
    let decision = body
        .find("finalization_assessment terminal_state")
        .expect("the decision is read");
    assert!(
        dirty < facts && prior < facts,
        "every blocker precedes the outcome read"
    );
    assert!(
        facts < decision,
        "the facts are gathered before the decision is read"
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

// Every state the validator may report, and every state it calls a success,
// must be one workflow-complete publishes.
#[test]
fn the_validators_vocabulary_stays_inside_the_published_one() {
    let published = tool_text("workflow_finalization_complete.sh");
    let result = tool_text("workflow_finalization_result.sh");
    let states = |function: &str| -> Vec<String> {
        let body = result
            .split_once(&format!("{function}() {{"))
            .unwrap_or_else(|| panic!("{function} must be defined"))
            .1;
        let guard = body
            .split_once("esac")
            .expect("the predicate must close its case")
            .0;
        guard
            .lines()
            .filter_map(|line| line.trim().strip_suffix(") return 0 ;;"))
            .flat_map(|names| names.split('|'))
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty() && name != "${1-}")
            .collect()
    };
    let allowed = states("_finalization_state_allowed");
    let successes = states("_finalization_state_is_success");
    assert!(
        allowed.len() >= 8,
        "the allowed vocabulary must not have quietly shrunk: {allowed:?}"
    );
    for state in allowed.iter().chain(successes.iter()) {
        assert!(
            published.contains(&format!("\"{state}\"")),
            "`{state}` is not in workflow-complete's terminal_vocabulary"
        );
    }
    for state in &successes {
        assert!(
            allowed.contains(state),
            "`{state}` is a success but not an allowed state"
        );
    }
}

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
