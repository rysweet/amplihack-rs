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
//! 2. `structural_refusal` is set by a FAILED step's guard exit, never by
//!    captured refusal phrases, the runner's stderr,
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
            completed("step-05a-outcome-word", "ok"),
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
    assert!(!WORK_MARKERS.contains(&STRUCTURAL_REFUSAL_MARKER));
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
    let verdict = classify_run_failure(
        &run(vec![failed(
            "step-02",
            &format!("Command failed (exit 79): {GUARD_LINE}"),
            &[],
        )]),
        "",
    );
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
fn quoted_phrases_in_any_case_cannot_refuse_a_step() {
    for line in [
        "blocked_terminal orchestration_unavailable: width cap",
        "Blocked_Terminal ORCHESTRATION_UNAVAILABLE: width cap",
    ] {
        let verdict = classify_run_failure(&run(vec![failed("step-02", "", &[line])]), "");
        assert!(!structural(&verdict), "{line:?} is captured output");
    }
}

/// The terminating guard status wins even when another work marker decides
/// the diagnostic signal.
#[test]
fn the_guard_status_counts_even_when_another_marker_decides_the_class() {
    let verdict = classify_run_failure(
        &run(vec![failed(
            "step-02",
            "amplihack copilot failed (exit 79)",
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
    assert_eq!(verdict.class, FailureClass::Indeterminate);
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

/// A descendant refusal followed by an API rejection must retain the actual
/// terminating cause and remain available for agentic evaluation.
#[test]
fn a_quoted_guard_line_before_a_later_api_failure_is_not_a_refusal() {
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
    assert_eq!(verdict.class, FailureClass::AgentApi);
    assert!(!structural(&verdict));
}

// --- the marker is additive --------------------------------------------------

#[test]
fn the_marker_only_gains_structural_refusal() {
    assert_eq!(
        FAILURE_CLASS_MARKER_PREFIX,
        "amplihack.recipe.failure_class "
    );
    let verdict = classify_run_failure(
        &run(vec![failed(
            "step-02",
            &format!("Command failed (exit 79): {GUARD_LINE}"),
            &[],
        )]),
        "",
    );
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

/// The exit status is authoritative, regardless of diagnostic wording.
#[test]
fn a_guard_exit_without_a_guard_phrase_is_still_a_refusal() {
    for error in [
        "Command failed (exit 79): ",
        "amplihack copilot failed (exit 79)",
        "amplihack claude failed (exit 79)\nAPI Error: 529 Overloaded",
    ] {
        let verdict = classify_run_failure(&run(vec![failed("step-02", error, &[])]), "");
        assert_eq!(verdict.class, FailureClass::Work);
        assert!(
            structural(&verdict),
            "{error:?} must set structural_refusal"
        );
        assert!(!verdict.class.is_mechanically_retryable());
    }
}

#[test]
fn truncation_preserves_termination_provenance_for_both_guard_and_api_exits() {
    for exit in [1, 79] {
        for field in ["error", "output", "recent_stdout", "recent_stderr"] {
            for preceding in ["".to_string(), "x".repeat(2100)] {
                let evidence = format!("{preceding}{GUARD_LINE}\nExecution failed: 400");
                let header = format!("amplihack copilot failed (exit {exit})");
                let mut step = failed("step-02", &header, &[]);
                match field {
                    "error" => step.error = format!("{header}\n{evidence}"),
                    "output" => step.output = evidence,
                    "recent_stdout" => step.recent_stdout = vec![evidence],
                    "recent_stderr" => step.recent_stderr = vec![evidence],
                    _ => unreachable!(),
                }
                let result = run(vec![step]);
                // Runner stderr can elide the entire step diagnostic; it cannot
                // manufacture or clear an authoritative guard termination.
                for stderr in [
                    String::new(),
                    "x".repeat(1800),
                    "API Error: 529 Overloaded".repeat(120),
                ] {
                    let verdict = classify_run_failure(&result, &stderr);
                    assert_eq!(
                        structural(&verdict),
                        exit == 79,
                        "exit {exit}, {field}, stderr length {}",
                        stderr.len()
                    );
                    if exit == 79 {
                        assert_eq!(verdict.class, FailureClass::Work);
                        assert!(!verdict.class.is_mechanically_retryable());
                        assert!(verdict.reasoning().contains("FINAL"));
                    }
                    if !preceding.is_empty() || stderr.len() > 2000 {
                        assert!(!verdict.evidence.contains("[step-02]"));
                        assert!(verdict.evidence.starts_with("…(earlier evidence elided)…"));
                    }
                    assert_eq!(verdict.evidence.contains(GUARD_LINE), stderr.len() < 2000);
                }
            }
        }
    }
}

#[test]
fn termination_status_is_read_only_from_the_runners_error_envelope() {
    for (error, code) in [
        ("Command failed (exit 79): refused", Some(79)),
        (
            "Step 'step-02' failed: bash step failed: Command failed (exit 79): refused",
            Some(79),
        ),
        (
            "Step 'step-02' failed: agent step failed: amplihack copilot failed (exit 79)\nquoted output",
            Some(79),
        ),
        (
            "Step 'step-02' failed: bash step failed: Command failed (exit 1): Command failed (exit 79): quoted",
            Some(1),
        ),
        (
            "Step 'other-step' failed: bash step failed: Command failed (exit 79): quoted",
            None,
        ),
        (
            "Command failed (exit 1): Command failed (exit 79): quoted",
            Some(1),
        ),
        (
            "amplihack copilot failed (exit 79)\nquoted output",
            Some(79),
        ),
        (
            "amplihack claude failed (exit 1)\namplihack copilot failed (exit 79)",
            Some(1),
        ),
        ("source.rs:42: Command failed (exit 79): quoted", None),
        (
            "Execution failed: 400\nCommand failed (exit 79): quoted",
            None,
        ),
        (GUARD_LINE, None),
        ("Command failed (exit 179): refused", Some(179)),
        ("Command failed (exit nope): refused", None),
        ("amplihack copilot failed (exit 79) quoted", None),
    ] {
        assert_eq!(
            failed("step-02", error, &[]).termination_exit_code(),
            code,
            "{error}"
        );
    }
}

/// recipe-runner-rs `90286bc` writes the step id in its envelope through
/// `escape_debug` (`diagnostic_identifier`). A guard refusal in a step whose id
/// needs escaping must still be read as exit 79, and the raw form an older
/// runner wrote must still decode.
#[test]
fn the_envelope_is_decoded_with_the_runners_escaped_step_id() {
    let id = "say \"hi\" \\ there";
    let escaped: String = id.escape_debug().collect();
    assert_ne!(
        escaped, id,
        "the id must need escaping for this test to mean anything"
    );
    for (error, code) in [
        (
            format!("Step '{escaped}' failed: bash step failed: Command failed (exit 79): refused"),
            Some(79),
        ),
        (
            format!("Step '{id}' failed: bash step failed: Command failed (exit 79): refused"),
            Some(79),
        ),
        (
            format!(
                "Step '{escaped}' failed: agent step failed: amplihack claude failed (exit 1)\nq"
            ),
            Some(1),
        ),
        (
            "Step 'say' failed: bash step failed: Command failed (exit 79): refused".to_string(),
            None,
        ),
    ] {
        assert_eq!(
            failed(id, &error, &[]).termination_exit_code(),
            code,
            "{error}"
        );
    }
}

#[test]
fn a_completed_step_with_exit_79_does_not_refuse_the_run() {
    let mut done = completed("step-01", GUARD_LINE);
    done.error = "Command failed (exit 79): handled".into();
    let verdict = classify_run_failure(
        &run(vec![done, failed("step-02", "Execution failed: 400", &[])]),
        "",
    );
    assert!(!structural(&verdict));
}

// --- quoted guard text vetoes a retry (PR #1442 review, round 3) -------------

/// Text cannot set `structural_refusal`, but it stops a mechanical retry. A
/// refusal the envelope decoder cannot see (a later entry in a sub-recipe's
/// failed-step list, see `termination.rs`) otherwise reads as whatever else is
/// in the evidence, and a 529 from an earlier round would retry the whole run
/// into the guard.
#[test]
fn a_quoted_guard_line_vetoes_a_transport_retry_without_refusing() {
    for (error, stdout) in [
        (
            "amplihack copilot failed (exit 1)",
            vec![GUARD_LINE, "API Error: 529 Overloaded"],
        ),
        (
            "Command failed (exit 1): API Error: 529 Overloaded",
            vec![GUARD_LINE],
        ),
    ] {
        let verdict = classify_run_failure(&run(vec![failed("step-02", error, &stdout)]), "");
        assert_eq!(verdict.class, FailureClass::Indeterminate, "{error}");
        assert_eq!(verdict.signal, Some(STRUCTURAL_REFUSAL_MARKER));
        assert!(!verdict.class.is_mechanically_retryable());
        assert!(!structural(&verdict), "the veto never sets the flag");
        let why = verdict.reasoning();
        assert!(
            why.contains("never decides a refusal") && why.contains("Not retried"),
            "{why}"
        );
        assert!(!why.contains("FINAL"), "{why}");
    }
}

/// The veto is read before truncation, so 529s in the runner's stderr that
/// push the guard line out of the 2,000-character window cannot retry it.
#[test]
fn the_veto_survives_truncation_by_runner_stderr() {
    let result = run(vec![failed(
        "step-02",
        "amplihack copilot failed (exit 1)",
        &[GUARD_LINE],
    )]);
    let verdict = classify_run_failure(&result, &"API Error: 529 Overloaded".repeat(120));
    assert!(!verdict.evidence.contains(GUARD_LINE));
    // What is left in the window reads as transport; the veto is what stops it.
    assert_eq!(
        classify_failure_text(&verdict.evidence).0,
        FailureClass::TransientTransport
    );
    assert_eq!(verdict.class, FailureClass::Indeterminate);
    assert!(!verdict.class.is_mechanically_retryable());
    assert!(!structural(&verdict));
}

/// Only the transport verdict changes. A bare word is not the guard's line, a
/// completed step's output and the runner's stderr are not a failed step's
/// evidence, and every other class was never retried.
#[test]
fn the_veto_is_narrow() {
    let transient = |steps, stderr: &str| classify_run_failure(&run(steps), stderr).class;
    assert_eq!(
        transient(
            vec![failed(
                "s",
                "API Error: 529 Overloaded",
                &["BLOCKED_TERMINAL"]
            )],
            ""
        ),
        FailureClass::TransientTransport
    );
    assert_eq!(
        transient(
            vec![
                completed("s1", GUARD_LINE),
                failed("s2", "API Error: 529 Overloaded", &[])
            ],
            ""
        ),
        FailureClass::TransientTransport
    );
    assert_eq!(
        transient(
            vec![failed("s", "API Error: 529 Overloaded", &[])],
            GUARD_LINE
        ),
        FailureClass::TransientTransport
    );
    for (text, class) in [
        ("test result: FAILED", FailureClass::Work),
        ("Execution failed: 400", FailureClass::AgentApi),
        ("API Error: 401", FailureClass::Environmental),
    ] {
        let verdict = classify_run_failure(&run(vec![failed("s", text, &[GUARD_LINE])]), "");
        assert_eq!(verdict.class, class, "{text}");
        assert_ne!(verdict.signal, Some(STRUCTURAL_REFUSAL_MARKER), "{text}");
    }
}

#[test]
fn a_run_with_no_structured_result_is_vetoed_too() {
    let verdict = classify_error(&anyhow::anyhow!("{GUARD_LINE}\nAPI Error: 529 Overloaded"));
    assert_eq!(verdict.class, FailureClass::Indeterminate);
    assert!(!verdict.class.is_mechanically_retryable());
    assert!(!structural(&verdict));
    let control = classify_error(&anyhow::anyhow!("API Error: 529 Overloaded"));
    assert_eq!(control.class, FailureClass::TransientTransport);
}
