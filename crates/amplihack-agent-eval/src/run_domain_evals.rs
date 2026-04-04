//! Run domain agent evaluations across all domain agents.
//!
//! Ports Python `amplihack/eval/run_domain_evals.py`:
//! - Agent registry mapping names to constructors
//! - Per-agent eval execution with report collection
//! - Combined JSON report output

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Result for a single agent evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEvalResult {
    pub agent_name: String,
    pub overall_score: f64,
    pub overall_passed: bool,
    pub level_scores: HashMap<String, f64>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Combined evaluation report for all domain agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainEvalReport {
    pub results: HashMap<String, AgentEvalResult>,
    pub summary: String,
}

/// Known domain agent names.
pub const AGENT_NAMES: [&str; 5] = [
    "code_review",
    "meeting_synthesizer",
    "document_creator",
    "data_analysis",
    "project_planning",
];

/// Run evaluations for specified (or all) domain agents.
///
/// This is a structural port — actual eval execution requires concrete
/// agent implementations. Returns a report skeleton with agent names.
pub fn run_all_evals(
    agent_names: Option<&[&str]>,
    _output_dir: &str,
) -> DomainEvalReport {
    let names: Vec<&str> = agent_names
        .map(|n| n.to_vec())
        .unwrap_or_else(|| AGENT_NAMES.to_vec());

    let mut results = HashMap::new();

    for name in &names {
        if !AGENT_NAMES.contains(name) {
            tracing::warn!("Unknown agent '{}', skipping", name);
            continue;
        }

        results.insert(
            name.to_string(),
            AgentEvalResult {
                agent_name: name.to_string(),
                overall_score: 0.0,
                overall_passed: false,
                level_scores: HashMap::new(),
                error: Some(
                    "Structural port: concrete eval requires agent implementation"
                        .to_string(),
                ),
            },
        );
    }

    let passed = results
        .values()
        .filter(|r| r.overall_passed)
        .count();
    let total = results.len();
    let summary = format!("{passed}/{total} agents passed evaluation");

    DomainEvalReport { results, summary }
}

/// Format a summary line for a single agent result.
pub fn format_agent_summary(result: &AgentEvalResult) -> String {
    if let Some(err) = &result.error {
        format!("{}: ERROR - {}", result.agent_name, err)
    } else {
        let status = if result.overall_passed {
            "PASS"
        } else {
            "FAIL"
        };
        format!(
            "{}: {:.2}% [{}]",
            result.agent_name,
            result.overall_score * 100.0,
            status
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_evals_default() {
        let report = run_all_evals(None, "./test_output");
        assert_eq!(report.results.len(), 5);
        for name in AGENT_NAMES {
            assert!(report.results.contains_key(name));
        }
    }

    #[test]
    fn run_specific_agents() {
        let report =
            run_all_evals(Some(&["code_review", "meeting_synthesizer"]), "./test_output");
        assert_eq!(report.results.len(), 2);
    }

    #[test]
    fn unknown_agent_skipped() {
        let report = run_all_evals(Some(&["unknown_agent"]), "./test_output");
        assert_eq!(report.results.len(), 0);
    }

    #[test]
    fn format_summary_error() {
        let r = AgentEvalResult {
            agent_name: "test".to_string(),
            overall_score: 0.0,
            overall_passed: false,
            level_scores: HashMap::new(),
            error: Some("fail".to_string()),
        };
        let s = format_agent_summary(&r);
        assert!(s.contains("ERROR"));
    }

    #[test]
    fn format_summary_pass() {
        let r = AgentEvalResult {
            agent_name: "test".to_string(),
            overall_score: 0.85,
            overall_passed: true,
            level_scores: HashMap::new(),
            error: None,
        };
        let s = format_agent_summary(&r);
        assert!(s.contains("PASS"));
        assert!(s.contains("85.00"));
    }

    #[test]
    fn agent_names_constant() {
        assert_eq!(AGENT_NAMES.len(), 5);
        assert!(AGENT_NAMES.contains(&"code_review"));
        assert!(AGENT_NAMES.contains(&"project_planning"));
    }
}
