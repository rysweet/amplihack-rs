//! Evidence gathering for `failure_class` (issues #1267, #1390, #1435, #1437).
//!
//! Everything here reads text and returns text or a bit; the decision about
//! what the text means is made in `failure_class`.

use super::super::RecipeRunStepResult;

/// Trailing lines of a failed step's captured output that count as evidence.
/// The terminating error lives at the tail; earlier output describes work that
/// already happened and must not decide the class.
pub(super) const EVIDENCE_TAIL_LINES: usize = 40;

/// Upper bound on the evidence text handed to the marker/result payload.
const EVIDENCE_MAX_CHARS: usize = 2000;

/// The reset time a usage-limit message carries, if it states one (#1390).
/// Returned verbatim rather than parsed into a timestamp: the operator needs
/// to read it. The anchor lives with the other patterns (#1618).
pub(crate) fn usage_limit_reset_hint(text: &str) -> Option<String> {
    super::limit_patterns::reset_hint(text)
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

/// Does `text` quote the structural guard's own line, in any case?
///
/// Never a refusal (#1437): a handled descendant refusal is quoted the same
/// way. Used only to veto a mechanical retry (`failure_class`).
pub(super) fn quotes_structural_guard(text: &str) -> bool {
    text.to_ascii_lowercase()
        .contains(super::failure_markers::STRUCTURAL_REFUSAL_MARKER)
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

pub(crate) fn tail_lines(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

fn tail_of(lines: &[String], max_lines: usize) -> String {
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

pub(crate) fn truncate_evidence(mut text: String) -> String {
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
