//! The `assess-finalization` skip and the validator's exit set are one
//! contract written in two files (#2343, #2809).
//!
//! `workflow-finalize.yaml` skips the strong-model assessment for every
//! verdict its condition excludes; `classify_confirmed_outcome` ends the run
//! on every verdict in its own `case`. The two sets must be the same one. If
//! the validator stopped exiting on a verdict the recipe still skips, that
//! verdict would reach the fallback with no assessment and no complaint; if it
//! started exiting on one the recipe still runs, every such run would pay for
//! a strong-model call whose output is discarded. Neither failure is visible at
//! runtime, so it is asserted from the text of both files.

use std::collections::BTreeSet;

use crate::issue_1641_finalization_open_pr::fixture::{tool_text, workspace_root};

/// The `assess-finalization` step of `<recipe>.yaml`, from its `- id:` line to
/// the line before the next step.
fn assessment_step(recipe: &str) -> String {
    let text = std::fs::read_to_string(
        workspace_root().join(format!("amplifier-bundle/recipes/{recipe}.yaml")),
    )
    .unwrap_or_else(|e| panic!("read {recipe}.yaml: {e}"));
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(r#"- id: "assess-finalization""#))
        .unwrap_or_else(|| panic!("{recipe}.yaml must define an assess-finalization step"));
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with("- id:"))
        .map_or(lines.len(), |offset| start + 1 + offset);
    lines[start..end].join("\n")
}

/// Every `outcome.verdict != 'X'` clause of the `assess-finalization`
/// condition, i.e. the verdicts the step is skipped for.
fn skipped_verdicts() -> BTreeSet<String> {
    let step = assessment_step("workflow-finalize");
    let condition = step
        .lines()
        .find_map(|l| l.trim().strip_prefix("condition:"))
        .expect("assess-finalization must carry a condition")
        .to_owned();
    let marker = "finalization_evidence.outcome.verdict != '";
    condition
        .match_indices(marker)
        .map(|(at, _)| {
            let rest = &condition[at + marker.len()..];
            rest[..rest.find('\'').expect("closing quote")].to_owned()
        })
        .collect()
}

/// The verdicts `classify_confirmed_outcome` classifies and exits on, read
/// from its guard `case`.
fn exiting_verdicts() -> BTreeSet<String> {
    let tool = tool_text("workflow_finalization_outcome_classify.sh");
    let body = tool
        .split_once("classify_confirmed_outcome() {")
        .expect("classify_confirmed_outcome must be defined")
        .1;
    let guard = body
        .lines()
        .find_map(|l| l.trim().strip_prefix(r#"case "$outcome_verdict" in"#))
        .expect("classify_confirmed_outcome must guard on $outcome_verdict");
    guard
        .split_once(')')
        .expect("the guard must name its verdicts")
        .0
        .split('|')
        .map(|v| v.trim().to_owned())
        .collect()
}

#[test]
fn the_assessment_is_skipped_for_exactly_the_verdicts_the_validator_exits_on() {
    let skipped = skipped_verdicts();
    assert!(
        !skipped.is_empty(),
        "the condition must exclude at least one verdict"
    );
    assert_eq!(
        skipped,
        exiting_verdicts(),
        "workflow-finalize.yaml skips the assessment for {skipped:?}, but \
         classify_confirmed_outcome exits on {:?}; the two must be the same set, \
         see docs/reference/default-workflow-agentic-finalization.md#the-assessment-step",
        exiting_verdicts()
    );
}

/// The assessment can never end the run: the agent step inside the sub-recipe
/// and the outer `type: recipe` step that calls it both continue on error. The
/// outer one matters on its own, because a sub-recipe that fails to resolve or
/// hits its recursion limit never reaches the inner step at all.
#[test]
fn both_assessment_steps_continue_on_error() {
    for recipe in ["workflow-finalize", "workflow-finalization-assessment"] {
        let step = assessment_step(recipe);
        assert!(
            step.contains("continue_on_error: true"),
            "{recipe}.yaml's assess-finalization step must carry continue_on_error: true, \
             so the assessment never ends the run:\n{step}"
        );
    }
}
