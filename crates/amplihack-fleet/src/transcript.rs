//! Transcript analysis — pattern extraction from JSONL session logs.
//!
//! Matches Python `amplihack/fleet/transcript_analyzer.py`:
//! - Parse Claude Code JSONL transcript files
//! - Extract workflow compliance indicators
//! - Detect strategy keywords and agent invocations
//! - Generate summary reports

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;
use tracing::debug;

/// A parsed transcript entry from JSONL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub tool_use: Option<String>,
    #[serde(default)]
    pub timestamp: Option<f64>,
}

/// Analysis report for a single transcript.
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptReport {
    pub file_path: String,
    pub total_entries: usize,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub tool_calls: usize,
    pub workflow_compliance: WorkflowCompliance,
    pub strategy_keywords: HashMap<String, usize>,
    pub agent_invocations: Vec<String>,
}

/// Workflow compliance indicators.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WorkflowCompliance {
    pub has_planning_phase: bool,
    pub has_implementation_phase: bool,
    pub has_testing_phase: bool,
    pub has_review_phase: bool,
    pub compliance_score: f64,
}

/// Strategy keywords to detect in transcripts.
const STRATEGY_KEYWORDS: &[&str] = &[
    "investigate",
    "implement",
    "refactor",
    "test",
    "debug",
    "optimize",
    "review",
    "deploy",
    "document",
    "plan",
];

/// Workflow phase indicators.
const PLANNING_INDICATORS: &[&str] = &["plan", "approach", "strategy", "analyze", "investigate"];
const IMPLEMENTATION_INDICATORS: &[&str] = &["implement", "create", "add", "build", "write"];
const TESTING_INDICATORS: &[&str] = &["test", "verify", "assert", "check", "validate"];
const REVIEW_INDICATORS: &[&str] = &["review", "audit", "quality", "lint", "format"];

/// Parse a JSONL transcript file into entries.
pub fn parse_transcript(path: &Path) -> anyhow::Result<Vec<TranscriptEntry>> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut entries = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<TranscriptEntry>(trimmed) {
            Ok(entry) => entries.push(entry),
            Err(_) => {
                // Try parsing as a generic JSON value and extract role/content
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    let role = val
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let content = val
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    entries.push(TranscriptEntry {
                        role,
                        content,
                        tool_use: val
                            .get("tool_use")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        timestamp: val.get("timestamp").and_then(|v| v.as_f64()),
                    });
                }
            }
        }
    }
    debug!(entries = entries.len(), path = %path.display(), "Parsed transcript");
    Ok(entries)
}

/// Analyze a transcript and produce a report.
pub fn analyze_transcript(path: &Path) -> anyhow::Result<TranscriptReport> {
    let entries = parse_transcript(path)?;
    let total = entries.len();
    let user_msgs = entries.iter().filter(|e| e.role == "user").count();
    let asst_msgs = entries.iter().filter(|e| e.role == "assistant").count();
    let tool_calls = entries.iter().filter(|e| e.tool_use.is_some()).count();

    let all_content: String = entries
        .iter()
        .map(|e| e.content.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();

    let strategy_keywords = count_strategy_keywords(&all_content);
    let agent_invocations = detect_agent_invocations(&entries);
    let workflow_compliance = assess_workflow_compliance(&all_content);

    Ok(TranscriptReport {
        file_path: path.display().to_string(),
        total_entries: total,
        user_messages: user_msgs,
        assistant_messages: asst_msgs,
        tool_calls,
        workflow_compliance,
        strategy_keywords,
        agent_invocations,
    })
}

fn count_strategy_keywords(content: &str) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for keyword in STRATEGY_KEYWORDS {
        let count = content.matches(keyword).count();
        if count > 0 {
            counts.insert(keyword.to_string(), count);
        }
    }
    counts
}

fn detect_agent_invocations(entries: &[TranscriptEntry]) -> Vec<String> {
    let mut invocations = Vec::new();
    let agent_patterns = [
        "Task(",
        "task(",
        "explore(",
        "general-purpose",
        "code-review",
    ];
    for entry in entries {
        for pattern in &agent_patterns {
            if entry.content.contains(pattern) && !invocations.contains(&pattern.to_string()) {
                invocations.push(pattern.to_string());
            }
        }
    }
    invocations
}

fn assess_workflow_compliance(content: &str) -> WorkflowCompliance {
    let has_planning = PLANNING_INDICATORS.iter().any(|i| content.contains(i));
    let has_impl = IMPLEMENTATION_INDICATORS
        .iter()
        .any(|i| content.contains(i));
    let has_testing = TESTING_INDICATORS.iter().any(|i| content.contains(i));
    let has_review = REVIEW_INDICATORS.iter().any(|i| content.contains(i));

    let phases_present = [has_planning, has_impl, has_testing, has_review]
        .iter()
        .filter(|&&b| b)
        .count();
    let score = phases_present as f64 / 4.0;

    WorkflowCompliance {
        has_planning_phase: has_planning,
        has_implementation_phase: has_impl,
        has_testing_phase: has_testing,
        has_review_phase: has_review,
        compliance_score: score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_jsonl_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("transcript.jsonl");
        std::fs::write(
            &file,
            r#"{"role":"user","content":"Fix the bug"}
{"role":"assistant","content":"I'll investigate and implement a fix"}
{"role":"assistant","content":"Running tests","tool_use":"bash"}
"#,
        )
        .unwrap();
        let entries = parse_transcript(&file).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].role, "user");
        assert_eq!(entries[2].tool_use.as_deref(), Some("bash"));
    }

    #[test]
    fn analyze_produces_report() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.jsonl");
        std::fs::write(
            &file,
            r#"{"role":"user","content":"Plan and implement a new feature"}
{"role":"assistant","content":"Let me plan the approach first, then implement and test it"}
{"role":"assistant","content":"Running the test suite to verify","tool_use":"bash"}
{"role":"assistant","content":"Code review and quality audit complete"}
"#,
        )
        .unwrap();
        let report = analyze_transcript(&file).unwrap();
        assert_eq!(report.total_entries, 4);
        assert_eq!(report.user_messages, 1);
        assert_eq!(report.assistant_messages, 3);
        assert_eq!(report.tool_calls, 1);
        assert!(report.workflow_compliance.has_planning_phase);
        assert!(report.workflow_compliance.has_implementation_phase);
        assert!(report.workflow_compliance.has_testing_phase);
        assert!(report.workflow_compliance.has_review_phase);
        assert!((report.workflow_compliance.compliance_score - 1.0).abs() < 0.01);
    }

    #[test]
    fn strategy_keyword_counting() {
        let content = "investigate the bug, then implement a fix, test it, and test again";
        let counts = count_strategy_keywords(content);
        assert_eq!(counts.get("investigate"), Some(&1));
        assert_eq!(counts.get("implement"), Some(&1));
        assert_eq!(counts.get("test"), Some(&2));
        assert!(counts.get("deploy").is_none());
    }

    #[test]
    fn empty_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("empty.jsonl");
        std::fs::write(&file, "").unwrap();
        let entries = parse_transcript(&file).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn workflow_compliance_partial() {
        let content = "let me plan and implement but skip verification";
        let compliance = assess_workflow_compliance(content);
        assert!(compliance.has_planning_phase);
        assert!(compliance.has_implementation_phase);
        assert!(!compliance.has_testing_phase);
        assert!((compliance.compliance_score - 0.5).abs() < 0.01);
    }

    #[test]
    fn parse_transcript_malformed_lines_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bad.jsonl");
        std::fs::write(
            &file,
            "not json at all\n{\"role\":\"user\",\"content\":\"hello\"}\n{broken\n",
        )
        .unwrap();
        let entries = parse_transcript(&file).unwrap();
        assert_eq!(entries.len(), 1, "only valid JSON lines should be parsed");
        assert_eq!(entries[0].role, "user");
    }

    #[test]
    fn parse_transcript_missing_file() {
        let result = parse_transcript(std::path::Path::new("/nonexistent/path.jsonl"));
        assert!(result.is_err());
    }

    #[test]
    fn workflow_compliance_no_phases() {
        let compliance = assess_workflow_compliance("random text with no keywords");
        assert!(!compliance.has_planning_phase);
        assert!(!compliance.has_implementation_phase);
        assert!(!compliance.has_testing_phase);
        assert!(!compliance.has_review_phase);
        assert!((compliance.compliance_score - 0.0).abs() < 0.01);
    }

    #[test]
    fn transcript_report_serializes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.jsonl");
        std::fs::write(&file, "{\"role\":\"user\",\"content\":\"hi\"}\n").unwrap();
        let report = analyze_transcript(&file).unwrap();
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["total_entries"], 1);
        assert_eq!(json["user_messages"], 1);
    }
}
