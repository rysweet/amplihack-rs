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

use super::super::RecipeRunResult;
use super::failure_evidence::{
    EVIDENCE_TAIL_LINES, is_completed_status, repo_selection_repo_hint, step_evidence,
    structural_refusal_in, tail_lines, truncate_evidence,
};
use super::failure_markers::{
    AGENT_API_MARKERS, ENVIRONMENTAL_MARKERS, REPO_SELECTION_MARKERS, STRUCTURAL_REFUSAL_MARKER,
    TRANSIENT_MARKERS, USAGE_LIMIT_MARKERS, WORK_MARKERS,
};
use serde_json::{Value as JsonValue, json};
use std::io::Write;

pub(crate) use super::failure_evidence::usage_limit_reset_hint;

/// Greppable prefix for the classification marker written to stderr. A
/// supervisor tailing a run log can find every classification decision — and
/// the abnormal termination itself — with a single `grep`.
pub(crate) const FAILURE_CLASS_MARKER_PREFIX: &str = "amplihack.recipe.failure_class ";

/// Key under which the classification is attached to the run result so the
/// agentic layer can read it as data rather than scraping prose.
pub(crate) const FAILURE_CLASS_RESULT_KEY: &str = "failure_classification";

/// What kind of failure this was.
///
/// When evidence carries markers from more than one class, precedence is
/// `Work` > `Environmental` > `UsageLimit` > `AgentApi` > `TransientTransport` > `Indeterminate`
/// (see `classify_failure_text`). A 404 that carries an environmental marker
/// (`api error: 404`, `404 not found`, `not_found_error`, `model_not_found`)
/// is therefore `Environmental` (#1421), never `AgentApi` (#1437).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureClass {
    /// Unambiguously transient transport fault: the request never reached a
    /// verdict. Mechanically retryable.
    TransientTransport,
    /// The environment cannot support the work as configured (missing binary,
    /// no auth, disk full, no repository selected). Retrying the identical call
    /// cannot help; a human or an agent has to change something first.
    ///
    /// Issue #1435: a repository-selection refusal lands here, and its verdict
    /// repeats the remedy the refusal already stated rather than only naming
    /// the class.
    Environmental,
    /// The work itself failed: a test failed, code did not compile, an
    /// assertion blew up, a policy guard refused. Never mechanically retried.
    Work,
    /// An account usage / session limit was reached. Issue #1390: this is a
    /// *temporary* fault that names its own reset time, but it is NOT the same
    /// as a transport hiccup. The mechanical retry budget is measured in
    /// minutes; a usage limit resets hours later. Retrying inside the budget
    /// would burn every attempt and still fail, so this is not mechanically
    /// retryable — but calling it `Indeterminate` told the operator nothing was
    /// known when in fact the cause and its reset time were stated plainly.
    UsageLimit,
    /// The agent transport itself never reached a verdict: the agent / session
    /// endpoint rejected the request outright (`Execution failed: 400`, "the
    /// resource you requested was not found", a vanished session). Issue #1437: this used to
    /// fall through to `Indeterminate` — "nothing in the evidence identifies the
    /// failure" — about a message that names its own cause, and a supervising
    /// loop reading that as a structural refusal stopped a run whose agent had
    /// already committed and pushed real work.
    ///
    /// It is NOT mechanically retryable: the identical request gets the
    /// identical rejection. It is also NOT a structural refusal — nothing
    /// declined to do the work, a call failed — so a loop is free to decide, on
    /// its own evidence, whether another round is worthwhile.
    AgentApi,
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
            Self::UsageLimit => "usage_limit",
            Self::AgentApi => "agent_api",
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
    /// Did a STRUCTURAL GUARD refuse this run — the sealed recursion ceiling,
    /// the width cap, the free-memory floor (#1326/#1327/#1329/#1332)?
    ///
    /// This is the single bit a supervising loop needs to tell "stop, and never
    /// retry into the guard" from "a call failed; decide for yourself whether
    /// another round is worthwhile". It is published on the classification JSON
    /// as `structural_refusal` so a loop reads ONE boolean produced by the layer
    /// that knows, instead of grepping a whole transcript for a word that also
    /// appears in refusals a descendant already handled (issue #1437).
    ///
    /// It follows the guard's whole phrase in a FAILED step's own evidence, not
    /// the deciding signal: a failing test printed in the same tail does not
    /// hide the refusal. With no failed step (`classify_error`) it is `false`;
    /// the exit code is what says 79 there.
    pub(crate) fn is_structural_refusal(&self) -> bool {
        !self.failed_steps.is_empty() && structural_refusal_in(&self.evidence, &self.failed_steps)
    }

    /// One-line human summary naming the class and the deciding marker.
    pub(crate) fn reasoning(&self) -> String {
        // Issue #1435: this error already told the operator what to do — pick a
        // repository, or add an origin. Answering only with a class name throws
        // that away a second time, which is the defect itself.
        if let Some(signal) = self.signal
            && REPO_SELECTION_MARKERS.contains(&signal)
        {
            let which = repo_selection_repo_hint(&self.evidence)
                .map(|repo| format!(" The repository it was pointed at is {repo:?}."))
                .unwrap_or_default();
            return format!(
                "classified `environmental` — evidence contains {signal:?}: the workflow \
                 cannot select a repository to work in.{which} Retrying is pointless; the \
                 same repo_path is refused the same way every time. Remedy: point \
                 `repo_path` at the specific repository to work in — in a multi-repository \
                 workspace that is one of the nested checkouts the error lists — or add an \
                 `origin` remote to the repository already selected, which this workflow \
                 needs to resolve a base ref and push its work (issue #1323).",
            );
        }
        // Issue #1390: a usage limit states its own reset time. Quoting it is
        // the difference between "something went wrong" and "come back at
        // 9:20am"; the operator should not have to re-read the raw log.
        if self.class == FailureClass::UsageLimit {
            let when = usage_limit_reset_hint(&self.evidence)
                .map(|hint| format!("; it resets {hint}"))
                .unwrap_or_default();
            return format!(
                "classified `usage_limit` — evidence contains {:?}{when}. This is \
                 temporary and NOT a work failure, but it is not retried inside the \
                 mechanical budget: that budget is minutes long and a usage limit \
                 outlasts it. Wait for the reset and resume.",
                self.signal.unwrap_or("a usage limit marker")
            );
        }
        // Issue #1437: name the cause. "Stopped" without a reason is what made
        // this expensive to diagnose in the first place.
        if self.class == FailureClass::AgentApi {
            return format!(
                "classified `agent_api` — evidence contains {:?}. The agent transport \
                 rejected the request outright, so no verdict was ever reached. This is \
                 NOT a structural policy refusal and NOT a work failure; it is also not \
                 retried mechanically, because the identical request gets the identical \
                 rejection. A supervising loop may start a fresh round.",
                self.signal.unwrap_or("an agent endpoint rejection")
            );
        }
        if self.is_structural_refusal() {
            return format!(
                "classified `{}` — a structural guard refused this run \
                 ({STRUCTURAL_REFUSAL_MARKER:?}, #1326/#1327/#1329/#1332). This is a FINAL \
                 answer: retrying into the guard is precisely what the guard exists to stop.",
                self.class.as_str()
            );
        }
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
            // Issue #1437: the one bit a supervising loop needs. `true` ONLY
            // when a structural guard refused THIS run; a transcript that
            // merely quotes a refusal a descendant already handled is `false`.
            "structural_refusal": self.is_structural_refusal(),
            "failed_steps": self.failed_steps,
            "completed_steps": self.completed_steps,
            "evidence": self.evidence,
        })
    }
}

/// Classify a blob of failure evidence. Pure: same text in, same verdict out.
///
/// Precedence is `Work` > `Environmental` > `UsageLimit` > `AgentApi` >
/// `TransientTransport`.
/// Ambiguity therefore resolves toward NOT retrying, which is the cheap
/// direction to be wrong in.
pub(crate) fn classify_failure_text(text: &str) -> (FailureClass, Option<&'static str>) {
    let haystack = text.to_ascii_lowercase();

    if let Some(marker) = first_match(&haystack, WORK_MARKERS) {
        return (FailureClass::Work, Some(marker));
    }
    // Checked ahead of ENVIRONMENTAL_MARKERS inside the same tier (#1435): both
    // yield `Environmental`, but only this table's signals let the verdict
    // repeat the remedy the precondition already stated.
    if let Some(marker) = first_match(&haystack, REPO_SELECTION_MARKERS) {
        return (FailureClass::Environmental, Some(marker));
    }
    if let Some(marker) = first_match(&haystack, ENVIRONMENTAL_MARKERS) {
        return (FailureClass::Environmental, Some(marker));
    }
    // Before TRANSIENT_MARKERS: a usage-limit response frequently arrives
    // wrapped in a 429 envelope, and "retry in a moment" is the wrong reading
    // of it (#1390).
    if let Some(marker) = first_match(&haystack, USAGE_LIMIT_MARKERS) {
        return (FailureClass::UsageLimit, Some(marker));
    }
    // Before TRANSIENT_MARKERS: an agent-endpoint rejection is a stable answer,
    // not an overloaded one, and it must not be read as "retry in a moment"
    // (#1437). After the three above so a real work/environment/limit cause
    // still wins when both appear.
    if let Some(marker) = first_match(&haystack, AGENT_API_MARKERS) {
        return (FailureClass::AgentApi, Some(marker));
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
    write_failure_class_marker(&payload);
}

/// Write the classification attached to a failed result once more, after the
/// result table (issue #1437).
///
/// The table goes to stdout after `emit_failure_class_marker` has run, and a
/// step's multi-line output reaches it unindented, so a line in it can look
/// like a marker. A supervisor reads the LAST marker in a log, so the runner's
/// own classification has to be the last line it writes.
pub(crate) fn emit_final_failure_class_marker(result: &RecipeRunResult) {
    let Some(payload) = result.extra.get(FAILURE_CLASS_RESULT_KEY) else {
        return;
    };
    let _ = std::io::stdout().flush();
    write_failure_class_marker(payload);
}

fn write_failure_class_marker(payload: &JsonValue) {
    let line = format!(
        "{FAILURE_CLASS_MARKER_PREFIX}{}",
        serde_json::to_string(payload).expect("a serde_json::Value always serialises")
    );
    let _ = writeln!(std::io::stderr().lock(), "{line}");
}
