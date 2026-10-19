//! Evidence gathering for `failure_class` (issues #1267, #1390, #1435, #1437).
//!
//! Everything here reads text and returns text or a bit; the decision about
//! what the text means is made in `failure_class`.

use super::super::RecipeRunStepResult;
use super::failure_markers::STRUCTURAL_REFUSAL_MARKER;

/// Trailing lines of a failed step's captured output that count as evidence.
/// The terminating error lives at the tail; earlier output describes work that
/// already happened and must not decide the class.
pub(super) const EVIDENCE_TAIL_LINES: usize = 40;

/// Upper bound on the evidence text handed to the marker/result payload.
const EVIDENCE_MAX_CHARS: usize = 2000;

/// The reset time a usage-limit message carries, if it states one. Returned
/// verbatim rather than parsed into a timestamp: the operator needs to read it,
/// and inventing a timezone we were not given would be worse than quoting it.
pub(crate) fn usage_limit_reset_hint(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let at = lower.find("resets ")?;
    let rest = &text[at + "resets ".len()..];
    let hint: String = rest
        .chars()
        .take_while(|c| *c != '\n' && *c != '\r')
        .collect();
    let hint = hint.trim();
    if hint.is_empty() {
        None
    } else {
        Some(hint.to_string())
    }
}

/// The repository a repository-selection precondition refused, if it names one.
///
/// Returned verbatim for the same reason the reset hint is (#1390): the
/// operator has to recognise the path, and a normalised or re-rooted version of
/// it would be a different path than the one they passed as `repo_path`.
pub(super) fn repo_selection_repo_hint(text: &str) -> Option<String> {
    const NEEDLE: &str = "is a git repository but has no 'origin' remote";
    let line = text
        .lines()
        .find(|line| line.to_ascii_lowercase().contains(NEEDLE))?;
    let open = line.find('\'')?;
    let rest = &line[open + 1..];
    let close = rest.find('\'')?;
    let repo = rest[..close].trim();
    if repo.is_empty() {
        None
    } else {
        Some(repo.to_string())
    }
}

/// A step status the runner reports for work that finished successfully.
pub(super) fn is_completed_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "success" | "succeeded" | "completed" | "ok" | "passed" | "skipped"
    )
}

/// Collect the tail of everything a failed step emitted. The runner surfaces
/// the agent's own stdout here — which is where `API Error: 529 Overloaded`
/// actually lands; the step's `error` field only ever says
/// "amplihack claude failed (exit 1)".
pub(super) fn step_evidence(step: &RecipeRunStepResult) -> String {
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

pub(super) fn tail_lines(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

fn tail_of(lines: &[String], max_lines: usize) -> String {
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

pub(super) fn truncate_evidence(mut text: String) -> String {
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

/// Prefixes `classify_run_failure` puts on the evidence that is NOT a failed
/// step's own output. Both come after every step part.
const RUN_STATUS_HEADER: &str = "[run status] ";
const RUNNER_STDERR_HEADER: &str = "[runner stderr] ";

/// Does the guard's whole refusal phrase appear in a FAILED step's own part of
/// `evidence` (issue #1437)?
///
/// `classify_run_failure` writes each failed step's part as `[<step id>] …`,
/// then `[run status] …`, then `[runner stderr] …`. Only lines inside a step
/// part count. The runner's stderr tail holds the transcripts of every run it
/// started, so a refusal a descendant already handled lands there, and it is
/// skipped for good once its header is seen. Lines ahead of the first step
/// header — what is left when `truncate_evidence` elided the start — have no
/// known origin and do not count either. Matching is case-insensitive.
pub(super) fn structural_refusal_in(evidence: &str, failed_steps: &[String]) -> bool {
    let headers: Vec<String> = failed_steps.iter().map(|id| format!("[{id}] ")).collect();
    let mut in_step = false;
    for line in evidence.lines() {
        if line.starts_with(RUNNER_STDERR_HEADER) {
            return false;
        }
        if line.starts_with(RUN_STATUS_HEADER) {
            in_step = false;
            continue;
        }
        if headers
            .iter()
            .any(|header| line.starts_with(header.as_str()))
        {
            in_step = true;
        }
        if in_step
            && line
                .to_ascii_lowercase()
                .contains(STRUCTURAL_REFUSAL_MARKER)
        {
            return true;
        }
    }
    false
}
