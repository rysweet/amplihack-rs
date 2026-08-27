//! Tests for issue #1267 — transient-vs-terminal failure classification and
//! the bounded mechanical retry built on top of it.
//!
//! Two failure modes are covered with equal weight:
//!
//! 1. a transient transport fault (the 529 from the issue) must NOT end the run;
//! 2. a terminal failure must NOT be retried — an unbounded retry of an
//!    impossible step is its own bug, and the more expensive one.
//!
//! No test here waits out a real backoff: `run_with_transient_retry` takes its
//! sleep as a parameter, so the injected sleep records the delay and returns.

use super::super::{RecipeRunResult, RecipeRunStepResult};
use super::failure_class::{
    FAILURE_CLASS_RESULT_KEY, FailureClass, FailureVerdict, classify_error, classify_failure_text,
    classify_run_failure, usage_limit_reset_hint,
};
use super::retry::{AttemptOutcome, StopReason, TransientRetryLimits, run_with_transient_retry};
use amplihack_utils::backoff::BackoffPolicy;
use std::cell::{Cell, RefCell};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The verbatim tail the issue reported. Reproduced exactly so the test fails
/// if the real-world text ever stops classifying as transient.
const ISSUE_1267_STDOUT_TAIL: &str = "\
API Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a moment.
If it persists, check https://status.claude.com.";

fn step(id: &str, status: &str) -> RecipeRunStepResult {
    RecipeRunStepResult {
        step_id: id.to_string(),
        status: status.to_string(),
        ..RecipeRunStepResult::default()
    }
}

fn failed_step_with_stdout(id: &str, error: &str, stdout: &[&str]) -> RecipeRunStepResult {
    RecipeRunStepResult {
        step_id: id.to_string(),
        status: "error".to_string(),
        error: error.to_string(),
        recent_stdout: stdout.iter().map(|line| line.to_string()).collect(),
        ..RecipeRunStepResult::default()
    }
}

fn limits(max_attempts: u32, budget_secs: u64) -> TransientRetryLimits {
    TransientRetryLimits::new(
        max_attempts,
        BackoffPolicy::new(
            Duration::from_secs(10),
            2.0,
            Duration::from_secs(budget_secs),
        ),
    )
}

fn transient_verdict() -> FailureVerdict {
    FailureVerdict {
        class: FailureClass::TransientTransport,
        signal: Some("api error: 529"),
        evidence: ISSUE_1267_STDOUT_TAIL.to_string(),
        failed_steps: vec!["step-12-run-precommit".to_string()],
        completed_steps: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// classify_failure_text — the pure classifier
// ---------------------------------------------------------------------------

#[test]
fn issue_1267_529_text_classifies_as_transient_transport() {
    let (class, signal) = classify_failure_text(ISSUE_1267_STDOUT_TAIL);
    assert_eq!(class, FailureClass::TransientTransport);
    assert_eq!(signal, Some("api error: 529"));
    assert!(class.is_mechanically_retryable());
}

#[test]
fn transport_level_faults_classify_as_transient() {
    for text in [
        "API Error: 503 Service Unavailable",
        "API Error: 429 Too Many Requests",
        "api error: 500 internal server error",
        "502 Bad Gateway",
        "error sending request: connection reset by peer",
        "fetch failed: ECONNRESET",
        "connect ETIMEDOUT 160.0.0.1:443",
        "socket hang up",
        "request timed out",
        r#"{"type":"error","error":{"type":"overloaded_error"}}"#,
        r#"{"type":"error","error":{"type":"rate_limit_error"}}"#,
    ] {
        let (class, signal) = classify_failure_text(text);
        assert_eq!(
            class,
            FailureClass::TransientTransport,
            "expected transient for {text:?} (signal {signal:?})"
        );
    }
}

#[test]
fn work_failures_are_never_transient() {
    for text in [
        "test result: FAILED. 3 passed; 1 failed",
        "assertion failed: left == right",
        "thread 'main' panicked at src/lib.rs:10:5",
        "error[E0308]: mismatched types",
        "error: could not compile `amplihack-cli`",
        "Traceback (most recent call last):\n  AssertionError",
        "CONFLICT (content): Merge conflict in src/main.rs",
    ] {
        let (class, _) = classify_failure_text(text);
        assert_eq!(
            class,
            FailureClass::Work,
            "expected work failure for {text:?}"
        );
        assert!(
            !class.is_mechanically_retryable(),
            "a work failure must never be mechanically retried: {text:?}"
        );
    }
}

/// 401/403 look like HTTP faults but are STABLE server answers. Retrying them
/// burns the budget and never succeeds, so they must not be transient.
#[test]
fn auth_rejections_are_environmental_not_transient() {
    for text in [
        "API Error: 401 {\"type\":\"authentication_error\"}",
        "invalid x-api-key",
        "API Error: 403 Forbidden",
        "amplihack: command not found",
        "No space left on device (os error 28)",
    ] {
        let (class, _) = classify_failure_text(text);
        assert_eq!(
            class,
            FailureClass::Environmental,
            "expected environmental for {text:?}"
        );
        assert!(!class.is_mechanically_retryable());
    }
}

/// Ambiguity must resolve toward NOT retrying. A run whose evidence shows a
/// real test failure is terminal even when a 529 also appears in the same tail.
#[test]
fn work_marker_wins_over_a_transient_marker_in_the_same_evidence() {
    let mixed = "API Error: 529 Overloaded\nlater: test result: FAILED. 1 failed";
    let (class, _) = classify_failure_text(mixed);
    assert_eq!(class, FailureClass::Work);
    assert!(!class.is_mechanically_retryable());
}

#[test]
fn unrecognised_failure_is_indeterminate_and_not_retried() {
    let (class, signal) = classify_failure_text("the step did not work");
    assert_eq!(class, FailureClass::Indeterminate);
    assert_eq!(signal, None);
    assert!(
        !class.is_mechanically_retryable(),
        "\"we do not know what happened\" is not a licence to retry"
    );
}

#[test]
fn classification_is_case_insensitive() {
    let (class, _) = classify_failure_text("API ERROR: 529 OVERLOADED");
    assert_eq!(class, FailureClass::TransientTransport);
}

// ---------------------------------------------------------------------------
// classify_run_failure — classification over a structured run result
// ---------------------------------------------------------------------------

/// The issue's exact shape: five completed phases, one failed agent step whose
/// own `error` says only "exit 1" and whose captured stdout carries the 529.
#[test]
fn issue_1267_run_shape_classifies_transient_and_records_completed_phases() {
    let result = RecipeRunResult {
        success: false,
        step_results: vec![
            step("workflow-prep", "success"),
            step("workflow-worktree", "success"),
            step("workflow-design", "success"),
            step("workflow-tdd", "success"),
            step("workflow-refactor-review", "success"),
            failed_step_with_stdout(
                "step-12-run-precommit",
                "agent step failed: amplihack claude failed (exit 1)",
                &ISSUE_1267_STDOUT_TAIL.lines().collect::<Vec<_>>(),
            ),
        ],
        ..RecipeRunResult::default()
    };

    let verdict = classify_run_failure(&result, "");
    assert_eq!(verdict.class, FailureClass::TransientTransport);
    assert_eq!(verdict.signal, Some("api error: 529"));
    assert_eq!(verdict.failed_steps, vec!["step-12-run-precommit"]);
    assert_eq!(
        verdict.completed_steps,
        vec![
            "workflow-prep",
            "workflow-worktree",
            "workflow-design",
            "workflow-tdd",
            "workflow-refactor-review",
        ],
        "the phases that survived must be named, not silently discarded"
    );
    assert!(verdict.reasoning().contains("transient_transport"));
}

#[test]
fn a_failing_test_step_classifies_as_work_even_with_stderr_noise() {
    let result = RecipeRunResult {
        success: false,
        step_results: vec![
            step("workflow-prep", "success"),
            failed_step_with_stdout(
                "step-07-tests",
                "bash step failed (exit 101)",
                &["running 4 tests", "test result: FAILED. 3 passed; 1 failed"],
            ),
        ],
        ..RecipeRunResult::default()
    };
    let verdict = classify_run_failure(&result, "warning: unused variable");
    assert_eq!(verdict.class, FailureClass::Work);
    assert!(!verdict.class.is_mechanically_retryable());
}

/// Evidence only in the runner's stderr tail (the runner died before emitting a
/// per-step record) must still be classified.
#[test]
fn runner_stderr_tail_is_part_of_the_evidence() {
    let result = RecipeRunResult {
        success: false,
        ..RecipeRunResult::default()
    };
    let verdict = classify_run_failure(&result, "API Error: 503 Service Unavailable");
    assert_eq!(verdict.class, FailureClass::TransientTransport);
}

#[test]
fn verdict_json_names_the_class_the_signal_and_the_action() {
    let payload = transient_verdict().to_json(2, "retry");
    assert_eq!(payload["class"], "transient_transport");
    assert_eq!(payload["signal"], "api error: 529");
    assert_eq!(payload["action"], "retry");
    assert_eq!(payload["attempt"], 2);
    assert_eq!(payload["retryable"], true);
    assert!(
        payload["reasoning"]
            .as_str()
            .expect("reasoning is a string")
            .contains("529"),
        "the decision must carry its own reasoning: {payload}"
    );
    // The key the agentic layer reads is stable.
    assert_eq!(FAILURE_CLASS_RESULT_KEY, "failure_classification");
}

// ---------------------------------------------------------------------------
// run_with_transient_retry — the bounded mechanical retry
// ---------------------------------------------------------------------------

/// Drives the retry loop with an injected sleep that records the delay instead
/// of waiting it out. Returns the final value, the summary, and every backoff
/// the loop asked for; no real time passes.
fn drive<T>(
    limits: &TransientRetryLimits,
    attempt: impl FnMut(u32) -> AttemptOutcome<T>,
) -> (T, super::retry::RetrySummary, Vec<Duration>) {
    let slept: RefCell<Vec<Duration>> = RefCell::new(Vec::new());
    let (value, summary) = run_with_transient_retry(
        limits,
        attempt,
        |_verdict, _attempt, _delay| {},
        |delay| slept.borrow_mut().push(delay),
    );
    let slept = slept.into_inner();
    (value, summary, slept)
}

#[test]
fn a_transient_failure_is_retried_and_the_run_survives() {
    let (value, summary, slept) = drive(&limits(3, 600), |attempt| {
        if attempt == 1 {
            AttemptOutcome::Transient("529", transient_verdict())
        } else {
            AttemptOutcome::Final("ok")
        }
    });

    assert_eq!(value, "ok", "the retry must produce the successful result");
    assert_eq!(summary.attempts, 2);
    assert_eq!(
        slept.len(),
        1,
        "exactly one backoff between the two attempts"
    );
    assert!(summary.stop_reason.is_none());
}

/// The failure mode that matters as much as the retry: a terminal error must be
/// attempted exactly once. An unbounded retry of an impossible step is a bug.
#[test]
fn a_terminal_failure_is_never_retried() {
    let calls = Cell::new(0u32);
    let (value, summary, slept) = drive(&limits(5, 600), |_attempt| {
        calls.set(calls.get() + 1);
        AttemptOutcome::Final("compile error")
    });

    assert_eq!(calls.get(), 1, "a terminal failure must be attempted once");
    assert_eq!(value, "compile error");
    assert_eq!(summary.attempts, 1);
    assert!(
        slept.is_empty(),
        "a terminal failure must not wait on a backoff"
    );
    assert!(summary.stop_reason.is_none());
    assert!(summary.last_verdict.is_none());
}

#[test]
fn a_permanently_transient_failure_stops_at_the_attempt_cap() {
    let calls = Cell::new(0u32);
    let (value, summary, slept) = drive(&limits(3, 3600), |_attempt| {
        calls.set(calls.get() + 1);
        AttemptOutcome::Transient("529", transient_verdict())
    });

    assert_eq!(calls.get(), 3, "bounded: exactly max_attempts invocations");
    assert_eq!(value, "529", "the last failure is what the caller sees");
    assert_eq!(summary.attempts, 3);
    assert_eq!(slept.len(), 2, "two backoffs between three attempts");
    assert_eq!(
        summary.stop_reason,
        Some(StopReason::AttemptCap { max_attempts: 3 })
    );
    assert!(
        summary
            .stop_reason
            .expect("stop reason")
            .describe()
            .contains("attempt cap"),
        "the terminal error must name why it stopped"
    );
    assert!(
        summary.last_verdict.is_some(),
        "the terminal error must name the class it exhausted its budget on"
    );
}

#[test]
fn a_permanently_transient_failure_stops_when_the_time_budget_is_spent() {
    // Attempt cap high enough that only the 30s budget can stop the loop. The
    // budget is charged against the backoffs themselves, so the recorded
    // delays are the whole accounting and no real time passes.
    let (_value, summary, slept) = drive(&limits(50, 30), |_attempt| {
        AttemptOutcome::Transient("529", transient_verdict())
    });

    assert!(matches!(
        summary.stop_reason,
        Some(StopReason::TimeBudget { .. })
    ));
    assert!(
        summary.attempts < 50,
        "the time budget, not the attempt cap, must have stopped this"
    );
    let total: Duration = slept.iter().sum();
    assert!(
        total <= Duration::from_secs(30),
        "backoffs must not overshoot the budget, got {total:?}"
    );
}

#[test]
fn backoff_delays_grow_between_attempts() {
    let (_value, _summary, slept) = drive(&limits(4, 3600), |_attempt| {
        AttemptOutcome::Transient("529", transient_verdict())
    });
    assert_eq!(slept.len(), 3);
    // Equal jitter keeps each delay within [base/2, base] of a doubling base,
    // so successive delays are strictly separated even at the jitter extremes.
    assert!(
        slept[1] >= slept[0],
        "delay must not shrink: {:?} then {:?}",
        slept[0],
        slept[1]
    );
    assert!(slept[2] >= slept[1]);
}

#[test]
fn a_single_attempt_limit_disables_the_retry_entirely() {
    let calls = Cell::new(0u32);
    let (_value, summary, slept) = drive(&limits(1, 3600), |_attempt| {
        calls.set(calls.get() + 1);
        AttemptOutcome::Transient("529", transient_verdict())
    });
    assert_eq!(calls.get(), 1);
    assert!(slept.is_empty());
    assert_eq!(
        summary.stop_reason,
        Some(StopReason::AttemptCap { max_attempts: 1 })
    );
}

#[test]
fn on_retry_is_invoked_once_per_wait_with_the_verdict() {
    let seen: RefCell<Vec<(u32, FailureClass)>> = RefCell::new(Vec::new());
    let (_value, summary) = run_with_transient_retry(
        &limits(3, 3600),
        |attempt| {
            if attempt < 3 {
                AttemptOutcome::Transient("529", transient_verdict())
            } else {
                AttemptOutcome::Final("ok")
            }
        },
        |verdict, attempt, _delay| seen.borrow_mut().push((attempt, verdict.class)),
        |_delay| {},
    );

    assert_eq!(summary.attempts, 3);
    let seen = seen.into_inner();
    assert_eq!(
        seen,
        vec![
            (1, FailureClass::TransientTransport),
            (2, FailureClass::TransientTransport)
        ],
        "every retry must be announced with the attempt number and the class"
    );
}

/// Regression guard for the bug this design nearly shipped with.
///
/// The budget was originally charged against wall-clock time since the run
/// began. That is exactly backwards for the fault #1267 is about: the 529
/// arrives *hours* into a long workstream, so by the first failure the elapsed
/// time already dwarfs any sane budget and the very first `plan_next` would
/// refuse to retry — the feature would be dead on arrival in the one scenario
/// it exists for. `waited` counts backoff time ONLY.
#[test]
fn the_budget_counts_backoff_waiting_not_the_age_of_the_run() {
    let limits = limits(5, 300);

    // Six hours of real work done, nothing yet spent waiting: the retry the
    // issue asks for must still be planned.
    let delay = limits
        .plan_next(1, Duration::ZERO)
        .expect("a long-running run must still get its first retry");
    assert!(delay > Duration::ZERO);

    // Only actual backoff waiting consumes the budget.
    assert_eq!(
        limits.plan_next(1, Duration::from_secs(300)),
        Err(StopReason::TimeBudget {
            budget: Duration::from_secs(300)
        }),
        "a spent backoff budget stops the loop"
    );
}

/// The end-to-end shape of the same guarantee: an attempt that takes an
/// arbitrarily long time still gets retried, because only the injected sleep
/// moves the budget.
#[test]
fn a_transient_failure_hours_into_a_run_is_still_retried() {
    let (value, summary, slept) = drive(&limits(3, 300), |attempt| {
        // Each attempt models hours of real work before the blip.
        if attempt == 1 {
            AttemptOutcome::Transient("529 after 6 hours of work", transient_verdict())
        } else {
            AttemptOutcome::Final("ok")
        }
    });

    assert_eq!(value, "ok", "the run must survive a late transient blip");
    assert_eq!(summary.attempts, 2);
    assert_eq!(slept.len(), 1);
    assert!(summary.stop_reason.is_none());
}

#[test]
fn limits_never_collapse_to_zero_attempts() {
    let limits = TransientRetryLimits::new(
        0,
        BackoffPolicy::new(Duration::from_secs(1), 2.0, Duration::from_secs(60)),
    );
    assert_eq!(
        limits.max_attempts(),
        1,
        "at least one attempt is always made"
    );
}

// -------------------------------------------------------------------------
// Issue #1390 — a usage limit is a temporary, self-describing fault. It used
// to classify as `indeterminate` ("nothing in the evidence identifies the
// failure") even though the message names both its cause and its reset time.
// -------------------------------------------------------------------------

/// The literal message that terminated four agent sessions on 2026-08-27.
const OBSERVED_SESSION_LIMIT: &str = "Agent terminated early due to an API error: You've hit your session limit · resets 9:20am (UTC)";

/// Written to compile against the PRE-FIX code on purpose: it names only
/// symbols that already existed, so it is a genuine red test rather than a
/// compile error. Before the fix this failed at runtime with
/// `class=Indeterminate`.
#[test]
fn the_observed_session_limit_message_is_never_indeterminate() {
    let (class, _) = classify_failure_text(OBSERVED_SESSION_LIMIT);
    assert_ne!(
        class,
        FailureClass::Indeterminate,
        "the message names its own cause and reset time; classifying it as \
         \"nothing in the evidence identifies the failure\" is wrong. Issue #1390."
    );
}

#[test]
fn the_observed_session_limit_message_classifies_as_a_usage_limit() {
    let (class, signal) = classify_failure_text(OBSERVED_SESSION_LIMIT);
    assert_eq!(
        class,
        FailureClass::UsageLimit,
        "the reported message must classify as a usage limit, not as \
         `indeterminate`; issue #1390. signal={signal:?}"
    );
    assert_eq!(signal, Some("session limit"));
}

#[test]
fn a_usage_limit_is_not_mechanically_retried() {
    // Temporary is not the same as retryable-right-now. The mechanical budget
    // is minutes; a usage limit outlasts it, so retrying burns every attempt
    // and still fails.
    assert!(
        !FailureClass::UsageLimit.is_mechanically_retryable(),
        "a usage limit must not be retried inside the mechanical budget"
    );
    assert!(
        FailureClass::TransientTransport.is_mechanically_retryable(),
        "genuinely transient transport faults must stay retryable"
    );
}

#[test]
fn a_usage_limit_wrapped_in_a_429_is_still_a_usage_limit() {
    // Providers commonly wrap a usage limit in a rate-limit envelope. Reading
    // that as "retry in a moment" is exactly the wrong call.
    let (class, _) = classify_failure_text(
        "API error: 429 too many requests — you have hit your usage limit, resets 9:20am (UTC)",
    );
    assert_eq!(
        class,
        FailureClass::UsageLimit,
        "a usage limit wrapped in a 429 envelope must not be read as a \
         short-lived transport fault"
    );
}

#[test]
fn a_bare_rate_limit_keeps_its_existing_transient_meaning() {
    // Guard against the fix over-reaching: short-lived envelopes must not be
    // reclassified, or every 429 stops being retried.
    let (class, _) = classify_failure_text("API error: 429 too many requests");
    assert_eq!(
        class,
        FailureClass::TransientTransport,
        "a bare 429 is still a short-lived transport fault"
    );
}

#[test]
fn the_verdict_tells_the_operator_when_to_come_back() {
    let verdict = classify_error(&anyhow::anyhow!("{OBSERVED_SESSION_LIMIT}"));
    let reasoning = verdict.reasoning();
    assert!(
        reasoning.contains("9:20am"),
        "the reset time is stated in the message and must survive into the \
         verdict, so the operator does not have to re-read the raw log. \
         Got: {reasoning}"
    );
    assert!(
        reasoning.contains("usage_limit"),
        "the verdict must name the class. Got: {reasoning}"
    );
}

#[test]
fn a_message_without_a_reset_time_still_classifies() {
    // Not every provider states a reset. Missing detail must degrade the
    // message, never the classification.
    let (class, _) = classify_failure_text("Error: monthly limit reached for this account");
    assert_eq!(class, FailureClass::UsageLimit);
    assert_eq!(usage_limit_reset_hint("no reset stated here"), None);
}

#[test]
fn work_failures_still_beat_a_usage_limit_mention() {
    // Precedence must not regress: a real test failure that merely mentions a
    // limit in passing is still a work failure.
    let (class, _) = classify_failure_text(
        "test result: FAILED. 1 failed\nnote: the fixture mentions a usage limit",
    );
    assert_eq!(
        class,
        FailureClass::Work,
        "work failures keep top precedence; ambiguity must resolve toward not retrying"
    );
}
