//! Structured JSONL provenance logging for classification and routing decisions.
//!
//! Writes append-only JSONL records to `.claude/runtime/logs/` subdirectories.
//! Fail-open: logging errors are warned but never propagate to callers.

use chrono::Utc;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Maximum characters of the user prompt to include in the log entry.
const PROMPT_PREVIEW_MAX_CHARS: usize = 200;

/// A structured provenance log entry written as one JSON line.
#[derive(Debug, Clone, Serialize)]
pub struct ProvenanceEntry {
    pub timestamp: String,
    pub event: String,
    pub decision: String,
    pub reason: String,
    pub confidence: f64,
    pub keywords: Vec<String>,
    pub prompt_preview: String,
    pub prompt_length: usize,
}

impl ProvenanceEntry {
    /// Build a provenance entry for a classification or routing decision.
    ///
    /// `prompt_preview` is automatically truncated to [`PROMPT_PREVIEW_MAX_CHARS`].
    pub fn new(
        event: impl Into<String>,
        decision: impl Into<String>,
        reason: impl Into<String>,
        confidence: f64,
        keywords: Vec<String>,
        prompt: &str,
    ) -> Self {
        let preview_end = prompt
            .char_indices()
            .nth(PROMPT_PREVIEW_MAX_CHARS)
            .map_or(prompt.len(), |(i, _)| i);
        Self {
            timestamp: Utc::now().to_rfc3339(),
            event: event.into(),
            decision: decision.into(),
            reason: reason.into(),
            confidence,
            keywords,
            prompt_preview: prompt[..preview_end].to_string(),
            prompt_length: prompt.len(),
        }
    }
}

/// Resolve the JSONL log path under `base_dir`.
///
/// Returns `<base_dir>/.claude/runtime/logs/<subdirectory>/<filename>`.
/// Creates parent directories if they don't exist.
fn resolve_log_path(
    base_dir: &Path,
    subdirectory: &str,
    filename: &str,
) -> std::io::Result<PathBuf> {
    let dir = base_dir
        .join(".claude")
        .join("runtime")
        .join("logs")
        .join(subdirectory);
    fs::create_dir_all(&dir)?;
    Ok(dir.join(filename))
}

/// Append a provenance entry as a single JSON line to the given log file.
///
/// Fail-open: returns `Ok(())` even when the write fails — errors are logged
/// via `tracing::warn` but never propagated.
pub fn log_provenance(
    base_dir: &Path,
    subdirectory: &str,
    filename: &str,
    entry: &ProvenanceEntry,
) {
    if let Err(e) = try_log_provenance(base_dir, subdirectory, filename, entry) {
        warn!(
            error = %e,
            subdirectory,
            filename,
            "provenance logging failed (fail-open, continuing)"
        );
    }
}

fn try_log_provenance(
    base_dir: &Path,
    subdirectory: &str,
    filename: &str,
    entry: &ProvenanceEntry,
) -> std::io::Result<()> {
    let path = resolve_log_path(base_dir, subdirectory, filename)?;
    let mut line = serde_json::to_string(entry)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())
}

/// Default subdirectory for workflow classifier logs.
pub const CLASSIFIER_LOG_SUBDIR: &str = "workflow_classifier";

/// Default filename for classification decision logs.
pub const CLASSIFIER_LOG_FILE: &str = "classification_decisions.jsonl";

/// Default subdirectory for intent router logs.
pub const ROUTER_LOG_SUBDIR: &str = "intent_router";

/// Default filename for routing decision logs.
pub const ROUTER_LOG_FILE: &str = "routing_decisions.jsonl";

/// Convenience: log a classification decision.
pub fn log_classification(base_dir: &Path, entry: &ProvenanceEntry) {
    log_provenance(base_dir, CLASSIFIER_LOG_SUBDIR, CLASSIFIER_LOG_FILE, entry);
}

/// Convenience: log a routing decision.
pub fn log_routing_decision(base_dir: &Path, entry: &ProvenanceEntry) {
    log_provenance(base_dir, ROUTER_LOG_SUBDIR, ROUTER_LOG_FILE, entry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn log_creates_directories_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let entry = ProvenanceEntry::new(
            "classification",
            "DEFAULT_WORKFLOW",
            "keyword 'implement'",
            0.9,
            vec!["implement".into()],
            "implement a new feature",
        );
        log_classification(dir.path(), &entry);

        let log_path = dir
            .path()
            .join(".claude/runtime/logs/workflow_classifier/classification_decisions.jsonl");
        assert!(log_path.exists(), "log file should be created");
        let content = fs::read_to_string(&log_path).unwrap();
        assert!(!content.is_empty());
    }

    #[test]
    fn appends_multiple_entries() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3 {
            let entry = ProvenanceEntry::new(
                "classification",
                "DEFAULT_WORKFLOW",
                format!("reason {i}"),
                0.7 + (i as f64) * 0.1,
                vec![],
                &format!("request {i}"),
            );
            log_classification(dir.path(), &entry);
        }

        let log_path = dir
            .path()
            .join(".claude/runtime/logs/workflow_classifier/classification_decisions.jsonl");
        let content = fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3, "should have 3 JSONL lines");

        for line in &lines {
            let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(parsed.get("timestamp").is_some());
            assert!(parsed.get("event").is_some());
        }
    }

    #[test]
    fn fail_open_on_bad_path() {
        let bad_base = Path::new("/proc/nonexistent_provenance_test");
        let entry = ProvenanceEntry::new(
            "classification",
            "DEFAULT_WORKFLOW",
            "test",
            0.5,
            vec![],
            "test prompt",
        );
        // Must not panic — fail-open behavior
        log_classification(bad_base, &entry);
    }

    #[test]
    fn field_validation_in_entry() {
        let long_prompt = "x".repeat(500);
        let entry = ProvenanceEntry::new(
            "classification",
            "INVESTIGATION_WORKFLOW",
            "keyword 'investigate'",
            0.85,
            vec!["investigate".into(), "analyze".into()],
            &long_prompt,
        );

        assert_eq!(entry.event, "classification");
        assert_eq!(entry.decision, "INVESTIGATION_WORKFLOW");
        assert_eq!(entry.confidence, 0.85);
        assert_eq!(entry.keywords.len(), 2);
        assert_eq!(entry.prompt_length, 500);
        assert_eq!(
            entry.prompt_preview.len(),
            PROMPT_PREVIEW_MAX_CHARS,
            "prompt_preview must be truncated to {PROMPT_PREVIEW_MAX_CHARS} chars"
        );

        let json: serde_json::Value = serde_json::to_value(&entry).unwrap();
        for field in [
            "timestamp",
            "event",
            "decision",
            "reason",
            "confidence",
            "keywords",
            "prompt_preview",
            "prompt_length",
        ] {
            assert!(json.get(field).is_some(), "missing field: {field}");
        }
    }

    #[test]
    fn routing_decision_log_uses_correct_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let entry = ProvenanceEntry::new(
            "routing_decision",
            "Security",
            "security-related keywords",
            0.8,
            vec!["vulnerability".into()],
            "check for vulnerability",
        );
        log_routing_decision(dir.path(), &entry);

        let log_path = dir
            .path()
            .join(".claude/runtime/logs/intent_router/routing_decisions.jsonl");
        assert!(log_path.exists(), "routing log file should be created");
        let content = fs::read_to_string(&log_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["event"], "routing_decision");
        assert_eq!(parsed["decision"], "Security");
    }

    #[test]
    fn empty_prompt_produces_valid_entry() {
        let entry = ProvenanceEntry::new(
            "classification",
            "DEFAULT_WORKFLOW",
            "empty request",
            0.0,
            vec![],
            "",
        );
        assert_eq!(entry.prompt_preview, "");
        assert_eq!(entry.prompt_length, 0);

        let json_str = serde_json::to_string(&entry).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["prompt_preview"], "");
        assert_eq!(parsed["prompt_length"], 0);
    }
}
