//! Five Office Task Agents Experiment.
//!
//! Ports Python `amplihack/eval/five_agent_experiment.py`:
//! - AgentExperimentResult for per-agent results
//! - ExperimentReport for combined 5-agent report
//! - Registry of 5 domain agent types with teaching topics

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Registry entry for a domain agent in the experiment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRegistryEntry {
    pub name: String,
    pub teaching_topic: String,
    pub description: String,
}

/// The 5 office task agent registry.
pub fn agent_registry() -> Vec<AgentRegistryEntry> {
    vec![
        AgentRegistryEntry {
            name: "code_review".to_string(),
            teaching_topic: "security review".to_string(),
            description: "Reviews code for quality, security, and style issues"
                .to_string(),
        },
        AgentRegistryEntry {
            name: "meeting_synthesizer".to_string(),
            teaching_topic: "meeting synthesis".to_string(),
            description: "Synthesizes meeting transcripts into structured summaries"
                .to_string(),
        },
        AgentRegistryEntry {
            name: "document_creator".to_string(),
            teaching_topic: "document structure".to_string(),
            description: "Creates and evaluates structured documents".to_string(),
        },
        AgentRegistryEntry {
            name: "data_analysis".to_string(),
            teaching_topic: "trend detection".to_string(),
            description: "Analyzes data, detects trends, generates insights"
                .to_string(),
        },
        AgentRegistryEntry {
            name: "project_planning".to_string(),
            teaching_topic: "risk assessment".to_string(),
            description: "Decomposes projects, identifies dependencies, assesses risks"
                .to_string(),
        },
    ]
}

/// Result for a single agent in the experiment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExperimentResult {
    pub agent_name: String,
    pub domain: String,
    pub description: String,
    pub eval_overall_score: f64,
    pub eval_overall_passed: bool,
    pub eval_level_scores: HashMap<String, f64>,
    pub teaching_topic: String,
    pub lesson_plan: String,
    pub instruction: String,
    pub student_attempt: String,
    pub combined_score: f64,
}

impl AgentExperimentResult {
    /// Compute combined score (70% eval, 30% teaching).
    pub fn compute_combined(eval_score: f64, teaching_score: f64) -> f64 {
        0.7 * eval_score + 0.3 * teaching_score
    }
}

/// Complete report for all 5 agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentReport {
    pub agent_results: Vec<AgentExperimentResult>,
    pub overall_eval_score: f64,
    /// Note: In Python source, this equals overall_combined_score (both derived
    /// from per-agent combined_score). Kept for API compatibility.
    pub overall_teaching_score: f64,
    pub overall_combined_score: f64,
    pub all_passed: bool,
    pub summary: String,
}

impl ExperimentReport {
    /// Build a report from individual agent results.
    pub fn from_results(results: Vec<AgentExperimentResult>) -> Self {
        let eval_scores: Vec<f64> =
            results.iter().map(|r| r.eval_overall_score).collect();
        let combined_scores: Vec<f64> =
            results.iter().map(|r| r.combined_score).collect();

        let overall_eval = if eval_scores.is_empty() {
            0.0
        } else {
            eval_scores.iter().sum::<f64>() / eval_scores.len() as f64
        };
        let overall_combined = if combined_scores.is_empty() {
            0.0
        } else {
            combined_scores.iter().sum::<f64>() / combined_scores.len() as f64
        };
        let all_passed = results.iter().all(|r| r.eval_overall_passed);

        let passed_count = results.iter().filter(|r| r.eval_overall_passed).count();
        let summary = format!(
            "5 Office Task Agents Experiment: {}/{} agents passed eval. \
             Overall eval score: {:.2}%. Overall combined score: {:.2}%. \
             All agents {}.",
            passed_count,
            results.len(),
            overall_eval * 100.0,
            overall_combined * 100.0,
            if all_passed {
                "PASS"
            } else {
                "have failures"
            }
        );

        Self {
            agent_results: results,
            overall_eval_score: overall_eval,
            overall_teaching_score: overall_combined,
            overall_combined_score: overall_combined,
            all_passed,
            summary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_5_agents() {
        let reg = agent_registry();
        assert_eq!(reg.len(), 5);
        assert_eq!(reg[0].name, "code_review");
        assert_eq!(reg[4].name, "project_planning");
    }

    #[test]
    fn combined_score_weights() {
        let combined = AgentExperimentResult::compute_combined(1.0, 1.0);
        assert!((combined - 1.0).abs() < f64::EPSILON);

        let combined = AgentExperimentResult::compute_combined(1.0, 0.0);
        assert!((combined - 0.7).abs() < f64::EPSILON);

        let combined = AgentExperimentResult::compute_combined(0.0, 1.0);
        assert!((combined - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn report_from_empty() {
        let report = ExperimentReport::from_results(vec![]);
        assert!(report.all_passed);
        assert!((report.overall_eval_score).abs() < f64::EPSILON);
    }

    #[test]
    fn report_from_results() {
        let results = vec![
            AgentExperimentResult {
                agent_name: "code_review".to_string(),
                domain: "code_review".to_string(),
                description: "test".to_string(),
                eval_overall_score: 0.8,
                eval_overall_passed: true,
                eval_level_scores: HashMap::new(),
                teaching_topic: "security".to_string(),
                lesson_plan: "plan".to_string(),
                instruction: "instr".to_string(),
                student_attempt: "attempt".to_string(),
                combined_score: 0.75,
            },
            AgentExperimentResult {
                agent_name: "meeting_synthesizer".to_string(),
                domain: "meeting_synthesizer".to_string(),
                description: "test".to_string(),
                eval_overall_score: 0.6,
                eval_overall_passed: false,
                eval_level_scores: HashMap::new(),
                teaching_topic: "meetings".to_string(),
                lesson_plan: "".to_string(),
                instruction: "".to_string(),
                student_attempt: "".to_string(),
                combined_score: 0.42,
            },
        ];
        let report = ExperimentReport::from_results(results);
        assert!(!report.all_passed);
        assert!((report.overall_eval_score - 0.7).abs() < f64::EPSILON);
        assert!(report.summary.contains("1/2"));
    }

    #[test]
    fn result_serde_roundtrip() {
        let r = AgentExperimentResult {
            agent_name: "test".to_string(),
            domain: "test".to_string(),
            description: "desc".to_string(),
            eval_overall_score: 0.9,
            eval_overall_passed: true,
            eval_level_scores: HashMap::from([("L1".to_string(), 0.95)]),
            teaching_topic: "topic".to_string(),
            lesson_plan: "plan".to_string(),
            instruction: "instr".to_string(),
            student_attempt: "attempt".to_string(),
            combined_score: 0.85,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: AgentExperimentResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agent_name, "test");
    }

    #[test]
    fn report_serde_roundtrip() {
        let report = ExperimentReport::from_results(vec![]);
        let json = serde_json::to_string(&report).unwrap();
        let back: ExperimentReport = serde_json::from_str(&json).unwrap();
        assert!(back.all_passed);
    }
}
