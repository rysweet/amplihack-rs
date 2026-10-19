//! Tests for issue #1437 — an agent/API failure is not a structural refusal.
//!
//! The merge-ready builder agent of PR #1416 did its work, committed and
//! pushed, and then lost its Copilot session (`Execution failed: 400 The
//! resource you requested was not found.`). The classifier answered
//! `indeterminate`, and the supervising loop read a refusal that a descendant
//! had already handled as a refusal of the round, so it stopped for good.
//!
//! Three things are pinned here:
//!
//! 1. an agent-endpoint rejection is classified `agent_api`, below `Work`,
//!    `Environmental` and `UsageLimit`; a 404 for an unknown model stays
//!    `Environmental` (#1421);
//! 2. `structural_refusal` is set only by the guard's whole refusal phrase in a
//!    FAILED step's own evidence, never by the bare words, the runner's stderr,
//!    a completed step, or a run with no structured result;
//! 3. the marker JSON only gains a key: nothing existing is renamed.

use super::super::{RecipeRunResult, RecipeRunStepResult};
use super::failure_class::{
    FAILURE_CLASS_MARKER_PREFIX, FailureClass, FailureVerdict, classify_error,
    classify_failure_text, classify_run_failure,
};
use super::failure_markers::{
    AGENT_API_MARKERS, ENVIRONMENTAL_MARKERS, STRUCTURAL_REFUSAL_MARKER, WORK_MARKERS,
};

/// The literal tail from the incident.
const ISSUE_1437_TAIL: &str = "\
I completed the work, committed 9d521815 and pushed.
amplihack copilot failed (exit 1)
Execution failed: 400 The resource you requested was not found.";

/// The guard's own line (`terminal_block` in `execute.rs`).
const GUARD_LINE: &str = "BLOCKED_TERMINAL orchestration_unavailable: depth 3 of max 3 \
                          (issue #964/#1326).";

fn completed(id: &str, output: &str) -> RecipeRunStepResult {
    RecipeRunStepResult {
        step_id: id.to_string(),
        status: "success".to_string(),
        output: output.to_string(),
        ..RecipeRunStepResult::default()
    }
}

fn failed(id: &str, error: &str, stdout: &[&str]) -> RecipeRunStepResult {
    RecipeRunStepResult {
        step_id: id.to_string(),
        status: "error".to_string(),
        error: error.to_string(),
        recent_stdout: stdout.iter().map(|line| line.to_string()).collect(),
        ..RecipeRunStepResult::default()
    }
}

fn run(steps: Vec<RecipeRunStepResult>) -> RecipeRunResult {
    RecipeRunResult {
        success: false,
        step_results: steps,
        ..RecipeRunResult::default()
    }
}

fn structural(verdict: &FailureVerdict) -> bool {
    let payload = verdict.to_json(1, "terminal");
    let flag = payload["structural_refusal"]
        .as_bool()
        .unwrap_or_else(|| panic!("structural_refusal must be a JSON bool: {payload}"));
    assert_eq!(
        flag,
        verdict.is_structural_refusal(),
        "the JSON key and the method must agree"
    );
    flag
}

// --- agent_api ---------------------------------------------------------------

#[test]
fn the_incident_tail_is_agent_api_and_not_retryable() {
    let (class, signal) = classify_failure_text(ISSUE_1437_TAIL);
    assert_eq!(class, FailureClass::AgentApi);
    assert_eq!(signal, Some("execution failed: 400"));
    assert_eq!(class.as_str(), "agent_api");
    assert!(
        !class.is_mechanically_retryable(),
        "the identical request gets the identical rejection"
    );
}

#[test]
fn every_agent_endpoint_rejection_is_agent_api() {
    for (text, marker) in [
        ("Execution failed: 400 bad payload", "execution failed: 400"),
        ("Execution failed: 404 gone", "execution failed: 404"),
        (
            "The resource you requested was not found.",
            "the resource you requested was not found",
        ),
        ("API Error: 400 {\"error\":\"x\"}", "api error: 400"),
        ("HTTP/1.1 400 Bad Request", "400 bad request"),
        (
            "{\"type\":\"invalid_request_error\",\"message\":\"x\"}",
            "invalid_request_error",
        ),
        ("the agent reported: session not found", "session not found"),
        ("Conversation not found: c-123", "conversation not found"),
    ] {
        assert_eq!(
            classify_failure_text(text),
            (FailureClass::AgentApi, Some(marker)),
            "expected agent_api for {text:?}"
        );
    }
}

/// D1/D3: #1421 decided an unknown model is environmental. #1437 must not move
/// any 404 that carries an environmental marker into `agent_api`.
#[test]
fn an_api_error_404_stays_environmental() {
    for (text, marker) in [
        ("API Error: 404 Not Found", "api error: 404"),
        ("404 Not Found", "404 not found"),
        ("{\"type\":\"not_found_error\"}", "not_found_error"),
    ] {
        assert_eq!(
            classify_failure_text(text),
            (FailureClass::Environmental, Some(marker)),
            "{text:?} is an unknown model (#1421), not an agent_api rejection"
        );
    }
}

/// D2: `model_not_found` is the error type for an unknown model.
#[test]
fn model_not_found_is_environmental() {
    assert_eq!(
        classify_failure_text("error: model_not_found"),
        (FailureClass::Environmental, Some("model_not_found"))
    );
    assert!(ENVIRONMENTAL_MARKERS.contains(&"model_not_found"));
    assert!(!AGENT_API_MARKERS.contains(&"model_not_found"));
}

#[test]
fn the_agent_api_table_is_exactly_the_eight_endpoint_rejections() {
    let mut got: Vec<&str> = AGENT_API_MARKERS.to_vec();
    got.sort_unstable();
    let mut want = vec![
        "execution failed: 400",
        "execution failed: 404",
        "the resource you requested was not found",
        "api error: 400",
        "400 bad request",
        "invalid_request_error",
        "session not found",
        "conversation not found",
    ];
    want.sort_unstable();
    assert_eq!(got, want);
}

#[test]
fn precedence_is_work_then_environmental_then_usage_limit_then_agent_api_then_transport() {
    let api = "Execution failed: 400 x";
    for (text, class) in [
        (format!("test result: FAILED\n{api}"), FailureClass::Work),
        (
            format!("{api}\nAPI Error: 404 Not Found"),
            FailureClass::Environmental,
        ),
        (
            format!("{api}\nmodel_not_found"),
            FailureClass::Environmental,
        ),
        (
            format!("{api}\nusage limit reached"),
            FailureClass::UsageLimit,
        ),
        (
            format!("API Error: 529 Overloaded\n{api}"),
            FailureClass::AgentApi,
        ),
    ] {
        assert_eq!(classify_failure_text(&text).0, class, "for {text:?}");
    }
}

#[test]
fn the_precedence_doc_names_agent_api() {
    let source = include_str!("failure_class.rs");
    assert!(
        source.contains(
            "`Work` > `Environmental` > `UsageLimit` > `AgentApi` > `TransientTransport` > `Indeterminate`"
        ),
        "the precedence doc comment must list every class in order, AgentApi included"
    );
}

#[test]
fn the_agent_api_verdict_names_its_cause_and_is_not_a_refusal() {
    let verdict = classify_run_failure(
        &run(vec![
            completed("step-03-extract-merge-ready-verdict", "ok"),
            failed(
                "step-04-address-blockers",
                "amplihack copilot failed (exit 1)",
                &ISSUE_1437_TAIL.lines().collect::<Vec<_>>(),
            ),
        ]),
        "",
    );
    let reasoning = verdict.reasoning();
    assert!(reasoning.contains("agent_api"), "got: {reasoning}");
    assert!(
        reasoning.contains("execution failed: 400"),
        "got: {reasoning}"
    );
    let payload = verdict.to_json(1, "terminal");
    assert_eq!(payload["class"], "agent_api");
    assert_eq!(payload["retryable"], false);
    assert!(!structural(&verdict));
}

// --- the refusal phrase ------------------------------------------------------

#[test]
fn the_bare_words_are_no_longer_work_markers() {
    for word in ["blocked_terminal", "orchestration_unavailable"] {
        assert!(
            !WORK_MARKERS.contains(&word),
            "{word:?} alone must not be a work marker"
        );
    }
    assert_eq!(
        STRUCTURAL_REFUSAL_MARKER,
        "blocked_terminal orchestration_unavailable"
    );
    assert!(WORK_MARKERS.contains(&STRUCTURAL_REFUSAL_MARKER));
}

#[test]
fn a_bare_word_alone_is_neither_work_nor_a_refusal() {
    for text in [
        "exit 79 / BLOCKED_TERMINAL is TERMINAL",
        "status: orchestration_unavailable",
        "BLOCKED_TERMINAL\norchestration_unavailable",
    ] {
        assert_eq!(
            classify_failure_text(text),
            (FailureClass::Indeterminate, None),
            "{text:?} quotes a word, it is not the guard's line"
        );
        let verdict = classify_run_failure(&run(vec![failed("step-02", text, &[])]), "");
        assert!(!structural(&verdict), "{text:?} set structural_refusal");
    }
}

#[test]
fn the_guard_line_in_a_failed_step_is_a_final_work_refusal() {
    let verdict = classify_run_failure(&run(vec![failed("step-02", GUARD_LINE, &[])]), "");
    assert_eq!(verdict.class, FailureClass::Work);
    assert_eq!(verdict.signal, Some(STRUCTURAL_REFUSAL_MARKER));
    assert!(structural(&verdict));
    assert!(
        verdict.reasoning().contains("FINAL"),
        "got: {}",
        verdict.reasoning()
    );
}

#[test]
fn the_phrase_matches_whatever_its_case() {
    for line in [
        "blocked_terminal orchestration_unavailable: width cap",
        "Blocked_Terminal ORCHESTRATION_UNAVAILABLE: width cap",
    ] {
        let verdict = classify_run_failure(&run(vec![failed("step-02", "", &[line])]), "");
        assert!(structural(&verdict), "{line:?} must set structural_refusal");
    }
}

/// The flag follows the phrase in the failed step, not the deciding signal: a
/// failing test printed earlier in the same tail does not hide the refusal.
#[test]
fn the_phrase_counts_even_when_another_marker_decides_the_class() {
    let verdict = classify_run_failure(
        &run(vec![failed(
            "step-02",
            "",
            &["test result: FAILED. 1 failed", GUARD_LINE],
        )]),
        "",
    );
    assert_eq!(verdict.class, FailureClass::Work);
    assert_ne!(verdict.signal, Some(STRUCTURAL_REFUSAL_MARKER));
    assert!(structural(&verdict));
}

/// SR-9: the runner's stderr tail holds the transcripts of every run it
/// started, so a refusal a descendant handled lands there.
#[test]
fn the_phrase_in_the_runner_stderr_alone_is_not_a_refusal() {
    let verdict = classify_run_failure(
        &run(vec![failed(
            "step-04-address-blockers",
            "amplihack copilot failed (exit 1)",
            &["Execution failed: 400 The resource you requested was not found."],
        )]),
        &format!("nested child said:\n{GUARD_LINE}\nhandled inline"),
    );
    assert!(!structural(&verdict));
}

#[test]
fn the_phrase_in_a_completed_step_is_not_a_refusal() {
    let verdict = classify_run_failure(
        &run(vec![
            completed("step-01", GUARD_LINE),
            failed("step-02", "Execution failed: 400 x", &[]),
        ]),
        "",
    );
    assert_eq!(verdict.class, FailureClass::AgentApi);
    assert!(!structural(&verdict));
}

/// With no structured result there is no failed step to read, so the flag is
/// always false; the exit code is what says 79.
#[test]
fn a_run_with_no_structured_result_never_sets_the_flag() {
    let verdict = classify_error(&anyhow::anyhow!("{GUARD_LINE}"));
    assert_eq!(verdict.class, FailureClass::Work);
    assert!(!structural(&verdict));
}

/// The self-poisoning transcript from the incident: the agent read the repo's
/// own contract test and handled a descendant's refusal inline.
#[test]
fn a_transcript_that_quotes_the_word_is_classified_by_its_real_cause() {
    let verdict = classify_run_failure(
        &run(vec![failed(
            "step-04-address-blockers",
            "amplihack copilot failed (exit 1)",
            &[
                "grep BLOCKED_TERMINAL amplifier-bundle/recipes/tests/*.sh",
                "  test-issue-1337.sh:20:#   - exit 79 / BLOCKED_TERMINAL is TERMINAL",
                "a nested run was refused; I completed the step inline as instructed.",
                "Execution failed: 400 The resource you requested was not found.",
            ],
        )]),
        "",
    );
    assert_eq!(verdict.class, FailureClass::AgentApi);
    assert!(!structural(&verdict));
}

/// Known limit, pinned on purpose (docs/reference/auto-drive-to-merge.md,
/// "Limits"): a descendant's real guard line still in the failed step's tail
/// sets the flag even when the step later failed on a 400. The loop then stops
/// and escalates rather than retrying into the guard.
#[test]
fn a_real_guard_line_before_a_later_failure_still_sets_the_flag() {
    let verdict = classify_run_failure(
        &run(vec![failed(
            "step-04-address-blockers",
            "amplihack copilot failed (exit 1)",
            &[
                GUARD_LINE,
                "a nested run was refused; I completed the step inline as instructed.",
                "Execution failed: 400 The resource you requested was not found.",
            ],
        )]),
        "",
    );
    assert_eq!(verdict.class, FailureClass::Work);
    assert!(structural(&verdict));
}

// --- the marker is additive --------------------------------------------------

#[test]
fn the_marker_only_gains_structural_refusal() {
    assert_eq!(
        FAILURE_CLASS_MARKER_PREFIX,
        "amplihack.recipe.failure_class "
    );
    let verdict = classify_run_failure(&run(vec![failed("step-02", GUARD_LINE, &[])]), "");
    let payload = verdict.to_json(2, "terminal");
    let mut keys: Vec<&str> = payload
        .as_object()
        .expect("the payload is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "action",
            "attempt",
            "class",
            "completed_steps",
            "evidence",
            "failed_steps",
            "issue",
            "reasoning",
            "retryable",
            "schema_version",
            "signal",
            "structural_refusal",
        ]
    );
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["issue"], 1267);
    assert_eq!(payload["attempt"], 2);
    assert_eq!(payload["action"], "terminal");
    assert_eq!(payload["structural_refusal"], true);
}
