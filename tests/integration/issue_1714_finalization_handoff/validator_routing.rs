//! Static guard: the finalization validator routes exactly its hand-off exits
//! through the #1714 brick (both `BLOCKED_CI` exits, and the two reporting
//! exits and the default `FAILED_IMPLEMENTATION` exit, gated on a confirmed
//! open PR).

use super::fixture::*;

// T6: the dispatcher sources the brick and the open-PR check before the
// validator, and the validator routes both BLOCKED_CI exits, both
// reporting-failure exits and the default FAILED_IMPLEMENTATION exit (#1714
// case 2) through the brick, and nothing else.
#[test]
fn the_validator_routes_its_handoff_exits_through_the_brick() {
    let dispatcher = read(&tool_path("workflow_agentic_finalization.sh"));
    let brick_at = dispatcher
        .find(&format!(". \"$here/{BRICK}\""))
        .expect("the dispatcher must source the brick through $here");
    let validate_at = dispatcher
        .find(". \"$here/workflow_finalization_validate.sh\"")
        .expect("the dispatcher sources the validator");
    assert!(brick_at < validate_at, "brick before validator");
    let outcome_at = dispatcher
        .find(". \"$here/workflow_finalization_outcome.sh\"")
        .expect("the dispatcher sources the outcome reader");
    assert!(outcome_at < validate_at, "outcome reader before validator");

    let validator = read(&tool_path("workflow_finalization_validate.sh"));
    assert!(validator.lines().count() <= 300, "validator line budget");
    // Since #2343 the outcome-driven BLOCKED_CI exit lives in the outcome
    // reader beside the classification it belongs to; every other hand-off
    // exit is still the validator's own, and there are no others.
    let outcome = read(&tool_path("workflow_finalization_outcome.sh"));
    let calls: Vec<&str> = code_lines(&validator)
        .filter(|l| l.contains("finalization_fail_or_handoff"))
        .collect();
    assert_eq!(
        calls.len(),
        3,
        "the prior-probe BLOCKED_CI exit, the reporting-path FAILED_IMPLEMENTATION \
         exit and the default exit: {calls:?}"
    );
    assert!(
        calls[0].contains("finalization_fail_or_handoff \"$prior_terminal_state\""),
        "the prior-probe blocker is the validator's bare hand-off: {calls:?}"
    );
    assert!(
        calls[1].contains("finalization_fail_or_handoff_open_pr \"FAILED_IMPLEMENTATION\"")
            && calls[1].contains("reporting_failure=true"),
        "the reporting-path FAILED_IMPLEMENTATION hands off only a confirmed PR: {calls:?}"
    );
    let tries: Vec<&str> = code_lines(&validator)
        .filter(|l| l.contains("finalization_try_handoff_open_pr"))
        .collect();
    assert!(
        tries.len() == 1
            && tries[0].contains("finalization_try_handoff_open_pr \"FAILED_REPORTING\""),
        "only FAILED_REPORTING uses the bare try, so it keeps its own result otherwise: {tries:?}"
    );
    assert!(
        calls[2].contains("finalization_fail_or_handoff_open_pr \"FAILED_IMPLEMENTATION\""),
        "the last hand-off exit is the default FAILED_IMPLEMENTATION, and it hands off \
         only a PR the outcome read confirmed: {calls:?}"
    );
    let last_code = code_lines(&validator)
        .filter(|l| !l.trim().is_empty() && l.trim() != "}")
        .last()
        .expect("validator has code");
    assert!(
        last_code.contains("finalization_fail_or_handoff_open_pr \"FAILED_IMPLEMENTATION\""),
        "the outcome-gated hand-off is the validator's final default: {last_code}"
    );
    let outcome_exits: Vec<&str> = code_lines(&outcome)
        .filter(|l| l.contains("finalization_fail_or_handoff"))
        .collect();
    assert!(
        outcome_exits.len() == 1
            && outcome_exits[0].contains("finalization_fail_or_handoff \"BLOCKED_CI\""),
        "the outcome reader's only hand-off exit is its failing-check BLOCKED_CI: {outcome_exits:?}"
    );
}
