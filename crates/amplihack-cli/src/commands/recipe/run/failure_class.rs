//! Failure classification for a `recipe-runner-rs` run (issue #1267).
//!
//! # Why this exists
//!
//! A transient server-side API fault (`API Error: 529 Overloaded`) used to
//! terminate an entire recipe run. Every non-zero exit was treated as terminal,
//! so a blip that would have succeeded seconds later unwound hours of completed
//! phases.
//!
//! # What is mechanical and what is not
//!
//! Only ONE decision is made here, and it is deliberately the narrow, purely
//! mechanical one: *is this failure an unambiguously transient transport
//! fault?* An HTTP 529/503/429, a reset connection, a socket timeout — those
//! are transport-level facts, not judgement calls, and a table of literal
//! markers classifies them reliably.
//!
//! The judgement call — "is this run still making progress, or is it looping
//! uselessly / attempting something impossible?" — is explicitly NOT made here.
//! That question belongs to the agentic layer (`loop-health-evaluator.yaml`,
//! issue #1337), which stops and looks at the evidence rather than counting
//! integers. This module's job is to hand that layer an honest, structured
//! classification instead of an undifferentiated "exit 1".
//!
//! # Fail-safe direction
//!
//! When evidence carries both a transient marker and a real-work marker, the
//! verdict is NOT transient. Retrying an impossible step forever is its own
//! bug, and it is the more expensive one: a missed retry costs one run, an
//! unbounded retry costs the whole budget and hides the real failure.

use super::super::{RecipeRunResult, RecipeRunStepResult};
use serde_json::{Value as JsonValue, json};
use std::io::Write;

/// Greppable prefix for the classification marker written to stderr. A
/// supervisor tailing a run log can find every classification decision — and
/// the abnormal termination itself — with a single `grep`.
pub(crate) const FAILURE_CLASS_MARKER_PREFIX: &str = "amplihack.recipe.failure_class ";

/// Key under which the classification is attached to the run result so the
/// agentic layer can read it as data rather than scraping prose.
pub(crate) const FAILURE_CLASS_RESULT_KEY: &str = "failure_classification";

/// Trailing lines of a failed step's captured output that count as evidence.
/// The terminating error lives at the tail; earlier output describes work that
/// already happened and must not decide the class.
const EVIDENCE_TAIL_LINES: usize = 40;

/// Upper bound on the evidence text handed to the marker/result payload.
const EVIDENCE_MAX_CHARS: usize = 2000;

/// What kind of failure this was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureClass {
    /// Unambiguously transient transport fault: the request never reached a
    /// verdict. Mechanically retryable.
    TransientTransport,
    /// The environment cannot support the work as configured (missing binary,
    /// no auth, disk full). Retrying the identical call cannot help; a human or
    /// an agent has to change something first.
    Environmental,
    /// The work itself failed: a test failed, code did not compile, an
    /// assertion blew up, a policy guard refused. Never mechanically retried.
    Work,
    /// Nothing in the evidence identifies the failure. Treated as terminal —
    /// "we do not know" is not a licence to retry.
    Indeterminate,
}

impl FailureClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::TransientTransport => "transient_transport",
            Self::Environmental => "environmental",
            Self::Work => "work",
            Self::Indeterminate => "indeterminate",
        }
    }

    /// Only an unambiguously transient transport fault may be retried by code.
    /// Everything else goes to the agentic layer with its evidence intact.
    pub(crate) fn is_mechanically_retryable(self) -> bool {
        matches!(self, Self::TransientTransport)
    }
}

/// A classification plus the literal evidence that produced it, so the decision
/// is auditable rather than asserted.
#[derive(Debug, Clone)]
pub(crate) struct FailureVerdict {
    pub(crate) class: FailureClass,
    /// The literal marker that decided the class. `None` for `Indeterminate`.
    pub(crate) signal: Option<&'static str>,
    /// The tail of output the decision was made from.
    pub(crate) evidence: String,
    pub(crate) failed_steps: Vec<String>,
    pub(crate) completed_steps: Vec<String>,
}

impl FailureVerdict {
    /// One-line human summary naming the class and the deciding marker.
    pub(crate) fn reasoning(&self) -> String {
        match self.signal {
            Some(signal) => format!(
                "classified `{}` — evidence contains {signal:?}",
                self.class.as_str()
            ),
            None => format!(
                "classified `{}` — no transport, environmental, or work marker found in the failure evidence",
                self.class.as_str()
            ),
        }
    }

    pub(crate) fn to_json(&self, attempt: u32, action: &str) -> JsonValue {
        json!({
            "schema_version": 1,
            "issue": 1267,
            "class": self.class.as_str(),
            "signal": self.signal,
            "reasoning": self.reasoning(),
            "action": action,
            "attempt": attempt,
            "retryable": self.class.is_mechanically_retryable(),
            "failed_steps": self.failed_steps,
            "completed_steps": self.completed_steps,
            "evidence": self.evidence,
        })
    }
}

/// Markers that only a transport-level fault produces. Every entry is a status
/// line or a socket-level error string — none of them is something a compiler,
/// a test runner, or an agent's prose would emit as its own terminal message.
///
/// Note what is deliberately absent: 401/403. An auth rejection is a *stable*
/// server answer, not an overloaded one, and retrying it just burns budget.
/// It is classified `Environmental` below.
const TRANSIENT_MARKERS: &[&str] = &[
    // Anthropic / OpenAI style overload and rate-limit envelopes.
    "api error: 529",
    "529 overloaded",
    "overloaded_error",
    "api error: 503",
    "503 service unavailable",
    "api error: 502",
    "502 bad gateway",
    "api error: 500",
    "500 internal server error",
    "504 gateway timeout",
    "api error: 429",
    "429 too many requests",
    "rate_limit_error",
    "rate limit exceeded",
    "server_error",
    "service temporarily unavailable",
    // Transport / socket level.
    "connection reset by peer",
    "connection closed before message completed",
    "econnreset",
    "etimedout",
    "epipe",
    "socket hang up",
    "tls handshake timed out",
    "temporary failure in name resolution",
    "upstream connect error",
    "request timed out",
];

/// Markers that mean the environment cannot support the work as configured.
/// Retrying the identical invocation cannot help — something has to change
/// first — so these are terminal for the mechanical retry.
const ENVIRONMENTAL_MARKERS: &[&str] = &[
    "api error: 401",
    "401 unauthorized",
    "authentication_error",
    "invalid api key",
    "invalid x-api-key",
    "api error: 403",
    "403 forbidden",
    "permission_error",
    "credit balance is too low",
    "command not found",
    "no such file or directory",
    "permission denied",
    "no space left on device",
    "disk quota exceeded",
    "gh auth login",
    "not logged in",
];

/// Markers that mean the work itself failed. These win over everything: a run
/// whose evidence says a test failed is not retried because a 529 also appears
/// somewhere in the same tail.
const WORK_MARKERS: &[&str] = &[
    "test result: failed",
    "test failed",
    "tests failed",
    "assertion failed",
    "assertionerror",
    "panicked at",
    "error[e",
    "could not compile",
    "compilation failed",
    "compilation error",
    "traceback (most recent call last)",
    "syntaxerror",
    "merge conflict",
    "hook failed",
    // Terminal policy refusals from the recursion / width guards (#1326,
    // #1327, #1332). These are FINAL answers by construction; retrying into a
    // guard is precisely the behaviour those guards exist to stop.
    "blocked_terminal",
    "orchestration_unavailable",
];

/// Classify a blob of failure evidence. Pure: same text in, same verdict out.
///
/// Precedence is `Work` > `Environmental` > `TransientTransport`. Ambiguity
/// therefore resolves toward NOT retrying, which is the cheap direction to be
/// wrong in.
pub(crate) fn classify_failure_text(text: &str) -> (FailureClass, Option<&'static str>) {
    let haystack = text.to_ascii_lowercase();

    if let Some(marker) = first_match(&haystack, WORK_MARKERS) {
        return (FailureClass::Work, Some(marker));
    }
    if let Some(marker) = first_match(&haystack, ENVIRONMENTAL_MARKERS) {
        return (FailureClass::Environmental, Some(marker));
    }
    if let Some(marker) = first_match(&haystack, TRANSIENT_MARKERS) {
        return (FailureClass::TransientTransport, Some(marker));
    }
    (FailureClass::Indeterminate, None)
}

fn first_match(haystack: &str, markers: &[&'static str]) -> Option<&'static str> {
    markers
        .iter()
        .copied()
        .filter_map(|marker| haystack.find(marker).map(|at| (at, marker)))
        .min_by_key(|(at, marker)| (*at, std::cmp::Reverse(marker.len())))
        .map(|(_, marker)| marker)
}

/// A step status the runner reports for work that finished successfully.
fn is_completed_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "success" | "succeeded" | "completed" | "ok" | "passed" | "skipped"
    )
}

/// Collect the tail of everything a failed step emitted. The runner surfaces
/// the agent's own stdout here — which is where `API Error: 529 Overloaded`
/// actually lands; the step's `error` field only ever says
/// "amplihack claude failed (exit 1)".
fn step_evidence(step: &RecipeRunStepResult) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !step.error.trim().is_empty() {
        parts.push(step.error.trim().to_string());
    }
    if !step.output.trim().is_empty() {
        parts.push(tail_lines(&step.output, EVIDENCE_TAIL_LINES));
    }
    if !step.recent_stdout.is_empty() {
        parts.push(tail_of(&step.recent_stdout, EVIDENCE_TAIL_LINES));
    }
    if !step.recent_stderr.is_empty() {
        parts.push(tail_of(&step.recent_stderr, EVIDENCE_TAIL_LINES));
    }
    parts.join("\n")
}

fn tail_lines(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

fn tail_of(lines: &[String], max_lines: usize) -> String {
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

fn truncate_evidence(mut text: String) -> String {
    if text.chars().count() <= EVIDENCE_MAX_CHARS {
        return text;
    }
    let keep: String = text
        .chars()
        .skip(text.chars().count() - EVIDENCE_MAX_CHARS)
        .collect();
    text = format!("…(earlier evidence elided)…\n{keep}");
    text
}

/// Classify a structured run result the runner reported as failed.
///
/// Evidence is drawn from the FAILED steps only. A run's completed phases are
/// recorded separately so the marker names what survived — the issue's own
/// repro printed five completed phases and then discarded them silently.
pub(crate) fn classify_run_failure(result: &RecipeRunResult, stderr_tail: &str) -> FailureVerdict {
    let mut failed_steps = Vec::new();
    let mut completed_steps = Vec::new();
    let mut evidence_parts: Vec<String> = Vec::new();

    for step in &result.step_results {
        if is_completed_status(&step.status) {
            completed_steps.push(step.step_id.clone());
            continue;
        }
        failed_steps.push(step.step_id.clone());
        let evidence = step_evidence(step);
        if !evidence.trim().is_empty() {
            evidence_parts.push(format!("[{}] {evidence}", step.step_id));
        }
    }

    if let Some(status) = &result.status
        && !status.trim().is_empty()
    {
        evidence_parts.push(format!("[run status] {status}"));
    }
    if !stderr_tail.trim().is_empty() {
        evidence_parts.push(format!(
            "[runner stderr] {}",
            tail_lines(stderr_tail, EVIDENCE_TAIL_LINES)
        ));
    }

    let evidence = truncate_evidence(evidence_parts.join("\n"));
    let (class, signal) = classify_failure_text(&evidence);
    FailureVerdict {
        class,
        signal,
        evidence,
        failed_steps,
        completed_steps,
    }
}

/// Classify a failure that never produced a structured result (spawn failure,
/// unparseable stdout, a runner that died before emitting JSON). The error
/// chain is the only evidence there is, and it already carries the stderr tail.
pub(crate) fn classify_error(error: &anyhow::Error) -> FailureVerdict {
    let evidence = truncate_evidence(format!("{error:#}"));
    let (class, signal) = classify_failure_text(&evidence);
    FailureVerdict {
        class,
        signal,
        evidence,
        failed_steps: Vec::new(),
        completed_steps: Vec::new(),
    }
}

/// Write the greppable classification marker. `action` is what the runner did
/// about it: `retry`, `terminal`, or `terminal_budget_exhausted`.
///
/// Written to stderr AND to `tracing`: stderr is what a supervisor tailing a
/// tmux pane can see, and issue #1267 called out that an abnormal termination
/// currently leaves nothing greppable behind at all.
pub(crate) fn emit_failure_class_marker(verdict: &FailureVerdict, attempt: u32, action: &str) {
    let payload = verdict.to_json(attempt, action);
    tracing::warn!(
        class = verdict.class.as_str(),
        signal = verdict.signal.unwrap_or("none"),
        action,
        attempt,
        "recipe run failure classified (issue #1267)"
    );
    let line = format!(
        "{FAILURE_CLASS_MARKER_PREFIX}{}",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
    );
    let stderr = std::io::stderr();
    let _ = writeln!(stderr.lock(), "{line}");
}
