//! The routing prompt's coordination wording, and the docs page that quotes
//! it (issue #1742).

use super::support::*;
use super::*;
use crate::workflow_classification::coordination::{
    EXPLICIT_WORK_REQUEST_PATTERN, VERB_LED_REPORT_PATTERN,
};

// ---- routing prompt ----

/// The routing prompt reaches every unmarked new-topic prompt in every
/// session, not only coordination traffic. Issue #1742 asked for one
/// exception to the DEV default (status, result and hand-over messages), not
/// for a narrower default. Rule 8 and the closing line must still send every
/// other ambiguous prompt to DEV.
#[test]
fn routing_prompt_keeps_the_dev_default_with_the_status_exception() {
    assert!(
        DEFAULT_ROUTING_PROMPT.contains(RULE_8),
        "rule 8 must read exactly as the docs quote it"
    );
    assert!(
        DEFAULT_ROUTING_PROMPT.contains("\n8. Ambiguous → **DEV** (safe default). "),
        "rule 8 must still open with the unqualified DEV default"
    );
    assert!(
        DEFAULT_ROUTING_PROMPT.contains(CLOSING_LINE),
        "the closing line must still read 'When in doubt, choose DEV.'"
    );
    for narrowed in [
        "Ambiguous request for code changes",
        "When a request for code changes is ambiguous",
    ] {
        assert!(
            !DEFAULT_ROUTING_PROMPT.contains(narrowed),
            "the DEV default must not be narrowed to code requests: {narrowed:?}"
        );
    }
}

#[test]
fn routing_prompt_has_the_coordination_calibration_lines() {
    assert!(
        DEFAULT_ROUTING_PROMPT.contains(CALIBRATION_BLOCK),
        "the five calibration lines must appear verbatim, two-space indent included"
    );
    let calibration = DEFAULT_ROUTING_PROMPT
        .find("Calibration examples")
        .expect("calibration header");
    let actions = DEFAULT_ROUTING_PROMPT
        .find("Actions:")
        .expect("actions header");
    let block = DEFAULT_ROUTING_PROMPT.find(CALIBRATION_BLOCK).unwrap();
    assert!(
        calibration < block && block < actions,
        "the new lines extend the existing calibration block"
    );
}

/// `VERB_LED_REPORT_PATTERN` and the docs rest on this order: the routing
/// prompt applies the first matching rule, and rule 4 sends an IMPLEMENT verb
/// to DEV before rule 8's exception for result reports is reached. A report
/// that starts with a change verb therefore has to be caught by the hook, not
/// left to rule 8 (review round 3 of PR #1752).
#[test]
fn routing_prompt_decides_implement_verbs_before_the_report_exception() {
    assert!(DEFAULT_ROUTING_PROMPT.contains("apply the FIRST matching rule"));
    let rule_4 = DEFAULT_ROUTING_PROMPT
        .find("\n4. FILE_EDIT or IMPLEMENT present (no UNDERSTAND) → **DEV**")
        .expect("rule 4 sends IMPLEMENT to DEV");
    let rule_8 = DEFAULT_ROUTING_PROMPT
        .find("\n8. Ambiguous → **DEV**")
        .expect("rule 8");
    assert!(rule_4 < rule_8);
}

#[test]
fn routing_prompt_keeps_its_existing_contract() {
    for kept in [
        "parallel signal evaluation",
        "flowchart TD",
        "UNDERSTAND + IMPLEMENT",
        "False positive costs minutes",
        "dev-orchestrator",
        "auto-intent-router",
        "Downstream invariant: Development classification always routes to \
         `default-workflow`; model-provided recipe fields do not override that invariant.",
    ] {
        assert!(DEFAULT_ROUTING_PROMPT.contains(kept), "lost {kept:?}");
    }
}

// ---- docs agree with the code ----

fn coordination_docs() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/reference/coordination-messages.md");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn docs_quote_the_note_and_calibration_verbatim() {
    let docs = coordination_docs();
    assert!(
        docs.contains(COORDINATION_NOTE),
        "docs must quote COORDINATION_NOTE"
    );
    assert!(
        docs.contains(CALIBRATION_BLOCK),
        "docs must quote the calibration block"
    );
    assert!(docs.contains(RULE_8), "docs must quote rule 8");
    assert!(
        docs.contains(CLOSING_LINE),
        "docs must quote the closing line"
    );
    assert!(
        docs.contains(EXPLICIT_WORK_REQUEST_PATTERN),
        "docs must quote the work-request pattern"
    );
    assert!(
        docs.contains(VERB_LED_REPORT_PATTERN),
        "docs must quote the verb-led report pattern"
    );
    assert!(docs.contains(COORDINATION_PREFIX));
}

/// The docs must not claim that unmarked prompts are unaffected: they receive
/// the amended routing prompt.
#[test]
fn docs_say_unmarked_prompts_get_the_amended_routing_prompt() {
    let docs = coordination_docs();
    assert!(
        !docs.contains("classified exactly as it was before this feature"),
        "an unmarked prompt receives the amended routing prompt"
    );
    assert!(docs.contains("every unmarked new-topic prompt"));
}

/// Review round 3 of PR #1752: rule 8 does not decide a report that starts
/// with a change verb, because rule 4 comes first. The docs must not say it
/// does, and must not promise that a note leaves the next prompt untouched.
#[test]
fn docs_do_not_rely_on_rule_8_for_verb_led_reports() {
    let docs = coordination_docs();
    for claim in [
        "whose rule 8 then classifies a result report as Q&A",
        "so the next human prompt is judged as it would have been without the coordination message",
    ] {
        assert!(!docs.contains(claim), "docs still claim {claim:?}");
    }
    assert!(docs.contains("rule 4"), "docs name the rule that decides");
}

#[test]
fn docs_list_every_last_rule_value() {
    let docs = coordination_docs();
    for rule in [
        "explicit_dev_command",
        "coordination_explicit_dev",
        "coordination_marker",
        "classification",
        "throttled",
    ] {
        assert!(docs.contains(&format!("`{rule}`")), "docs omit `{rule}`");
    }
    assert!(docs.contains(".claude/runtime/classification_state/"));
}
