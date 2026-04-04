//! SDK evaluation improvement loop.
//!
//! Ports Python `amplihack/evaluation/sdk_eval_loop.py`.
//! Runs N iterations of evaluate → analyse → recommend → re-evaluate per SDK,
//! tracking score progression and generating tuning recommendations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::EvalError;
use crate::models::ProgressiveResult;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Detailed info about a single test failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureDetail {
    pub level: String,
    pub failure_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
}

/// Result of one iteration in the improvement loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopIteration {
    pub iteration: u32,
    pub sdk: String,
    pub scores: HashMap<String, f64>,
    pub overall: f64,
    pub failures: Vec<FailureDetail>,
    pub recommendations: Vec<String>,
    pub duration_seconds: f64,
}

/// Full report for one SDK's improvement loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkEvalReport {
    pub sdk: String,
    pub iterations: Vec<LoopIteration>,
    pub final_scores: HashMap<String, f64>,
    pub final_overall: f64,
    pub score_progression: Vec<f64>,
    pub best_iteration: u32,
    pub best_overall: f64,
}

impl SdkEvalReport {
    pub fn new(sdk: impl Into<String>) -> Self {
        Self {
            sdk: sdk.into(),
            iterations: Vec::new(),
            final_scores: HashMap::new(),
            final_overall: 0.0,
            score_progression: Vec::new(),
            best_iteration: 0,
            best_overall: 0.0,
        }
    }

    pub fn add_iteration(&mut self, iter: LoopIteration) {
        if iter.overall > self.best_overall {
            self.best_overall = iter.overall;
            self.best_iteration = iter.iteration;
        }
        self.score_progression.push(iter.overall);
        self.final_scores = iter.scores.clone();
        self.final_overall = iter.overall;
        self.iterations.push(iter);
    }

    /// Whether scores improved from first to last iteration.
    pub fn improved(&self) -> bool {
        if self.score_progression.len() < 2 {
            return false;
        }
        let first = self.score_progression[0];
        let last = *self.score_progression.last().unwrap();
        last > first
    }

    /// Total improvement (last − first).
    pub fn total_improvement(&self) -> f64 {
        if self.score_progression.len() < 2 {
            return 0.0;
        }
        self.score_progression.last().unwrap() - self.score_progression[0]
    }
}

/// Comparative report across multiple SDKs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiSdkReport {
    pub sdk_reports: HashMap<String, SdkEvalReport>,
    pub ranking: Vec<(String, f64)>,
    pub timestamp: DateTime<Utc>,
}

impl MultiSdkReport {
    pub fn new() -> Self {
        Self {
            sdk_reports: HashMap::new(),
            ranking: Vec::new(),
            timestamp: Utc::now(),
        }
    }

    pub fn add_report(&mut self, report: SdkEvalReport) {
        let sdk = report.sdk.clone();
        let score = report.final_overall;
        self.sdk_reports.insert(sdk.clone(), report);
        self.ranking.push((sdk, score));
        self.ranking
            .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// Best SDK by final overall score.
    pub fn best_sdk(&self) -> Option<&str> {
        self.ranking.first().map(|(s, _)| s.as_str())
    }
}

impl Default for MultiSdkReport {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Failure analysis
// ---------------------------------------------------------------------------

/// Extract structured failure details from a progressive eval result.
pub fn analyze_failures(result: &ProgressiveResult) -> Vec<FailureDetail> {
    result
        .level_results
        .iter()
        .filter(|lr| !lr.success)
        .map(|lr| FailureDetail {
            level: lr.level_name.clone(),
            failure_type: "level_failure".to_string(),
            error: lr.error_message.clone(),
            score: if lr.scores.is_empty() {
                None
            } else {
                Some(lr.average_score())
            },
        })
        .collect()
}

/// Extract per-level average scores from a progressive result.
pub fn extract_level_scores(result: &ProgressiveResult) -> HashMap<String, f64> {
    result
        .level_results
        .iter()
        .map(|lr| (lr.level_name.clone(), lr.average_score()))
        .collect()
}

// ---------------------------------------------------------------------------
// Recommendation generation
// ---------------------------------------------------------------------------

/// Generate SDK-specific prompt-tuning recommendations from failures.
pub fn generate_recommendations(failures: &[FailureDetail], sdk: &str) -> Vec<String> {
    let mut recs = Vec::new();

    // Group failures by level
    let mut by_level: HashMap<&str, usize> = HashMap::new();
    for f in failures {
        *by_level.entry(f.level.as_str()).or_insert(0) += 1;
    }

    for (level, count) in &by_level {
        recs.push(format!(
            "[{sdk}] Level '{level}' had {count} failure(s) — review prompts for this cognitive level"
        ));
    }

    // Low-score recommendations
    for f in failures {
        if let Some(score) = f.score
            && score < 0.3
        {
            recs.push(format!(
                "[{sdk}] Very low score ({score:.2}) at level '{}' — \
                 consider restructuring retrieval strategy",
                f.level
            ));
        }
    }

    if recs.is_empty() {
        recs.push(format!(
            "[{sdk}] No specific recommendations — all levels acceptable"
        ));
    }

    recs
}

// ---------------------------------------------------------------------------
// Loop configuration
// ---------------------------------------------------------------------------

/// Configuration for the SDK eval improvement loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkEvalLoopConfig {
    pub sdks: Vec<String>,
    pub num_loops: u32,
    #[serde(default)]
    pub levels: Option<Vec<String>>,
    pub output_dir: String,
}

impl SdkEvalLoopConfig {
    pub fn new(sdks: Vec<String>, num_loops: u32) -> Self {
        Self {
            sdks,
            num_loops,
            levels: None,
            output_dir: "./eval_sdk_loop".to_string(),
        }
    }

    pub fn with_levels(mut self, levels: Vec<String>) -> Self {
        self.levels = Some(levels);
        self
    }

    pub fn with_output_dir(mut self, dir: impl Into<String>) -> Self {
        self.output_dir = dir.into();
        self
    }

    pub fn validate(&self) -> Result<(), EvalError> {
        if self.sdks.is_empty() {
            return Err(EvalError::config("at least one SDK is required"));
        }
        if self.num_loops == 0 {
            return Err(EvalError::config("num_loops must be > 0"));
        }
        for (i, sdk) in self.sdks.iter().enumerate() {
            if sdk.is_empty() {
                return Err(EvalError::config(format!("sdk[{i}] must not be empty")));
            }
        }
        Ok(())
    }
}

/// All known SDK identifiers.
pub const ALL_SDKS: &[&str] = &[
    "mini",
    "claude",
    "copilot",
    "microsoft",
    "multiagent-copilot",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::levels::TestLevel;
    use crate::models::{LevelResult, ProgressiveConfig};
    use std::path::PathBuf;

    fn make_progressive_result(pass: bool) -> ProgressiveResult {
        let config = ProgressiveConfig {
            output_dir: PathBuf::from("."),
            agent_name: "test".into(),
            levels_to_run: vec![TestLevel::L1Recall],
            memory_backend: "default".into(),
            sdk: "mini".into(),
            grader_votes: 1,
        };
        let mut result = ProgressiveResult::new(config);
        if pass {
            result.add_result(LevelResult::passed(TestLevel::L1Recall, vec![0.9, 0.8]));
        } else {
            result.add_result(LevelResult::failed(TestLevel::L1Recall, "too low"));
        }
        result
    }

    fn make_iter(sdk: &str, idx: u32, overall: f64) -> LoopIteration {
        LoopIteration {
            iteration: idx,
            sdk: sdk.into(),
            scores: HashMap::new(),
            overall,
            failures: Vec::new(),
            recommendations: Vec::new(),
            duration_seconds: 1.0,
        }
    }

    #[test]
    fn analyze_failures_and_scores() {
        let failed = make_progressive_result(false);
        assert_eq!(analyze_failures(&failed).len(), 1);
        assert!(analyze_failures(&make_progressive_result(true)).is_empty());
        let scores = extract_level_scores(&make_progressive_result(true));
        assert!(scores.contains_key("Recall"));
    }

    #[test]
    fn recommendations_generation() {
        let failures = vec![FailureDetail {
            level: "L1 Recall".into(),
            failure_type: "level_failure".into(),
            error: Some("low".into()),
            score: Some(0.2),
        }];
        let recs = generate_recommendations(&failures, "mini");
        assert!(recs.len() >= 2);
        assert!(recs.iter().any(|r| r.contains("[mini]")));
        let empty_recs = generate_recommendations(&[], "copilot");
        assert!(empty_recs[0].contains("No specific"));
    }

    #[test]
    fn sdk_eval_report_progression() {
        let mut report = SdkEvalReport::new("mini");
        report.add_iteration(make_iter("mini", 0, 0.5));
        report.add_iteration(make_iter("mini", 1, 0.7));
        assert!(report.improved());
        assert!((report.total_improvement() - 0.2).abs() < f64::EPSILON);
        assert_eq!(report.best_iteration, 1);
    }

    #[test]
    fn sdk_eval_report_no_improvement() {
        let mut report = SdkEvalReport::new("t");
        report.add_iteration(make_iter("t", 0, 0.8));
        report.add_iteration(make_iter("t", 1, 0.6));
        assert!(!report.improved());
        assert!(report.total_improvement() < 0.0);
    }

    #[test]
    fn sdk_eval_report_single_iteration() {
        let mut report = SdkEvalReport::new("t");
        report.add_iteration(make_iter("t", 0, 0.5));
        assert!(!report.improved());
        assert!((report.total_improvement()).abs() < f64::EPSILON);
    }

    #[test]
    fn multi_sdk_report_ranking() {
        let mut multi = MultiSdkReport::new();
        let mut r1 = SdkEvalReport::new("mini");
        r1.final_overall = 0.6;
        multi.add_report(r1);
        let mut r2 = SdkEvalReport::new("claude");
        r2.final_overall = 0.9;
        multi.add_report(r2);
        assert_eq!(multi.best_sdk(), Some("claude"));
        assert!(MultiSdkReport::new().best_sdk().is_none());
    }

    #[test]
    fn config_validation() {
        assert!(
            SdkEvalLoopConfig::new(vec!["mini".into()], 5)
                .validate()
                .is_ok()
        );
        assert!(SdkEvalLoopConfig::new(vec![], 5).validate().is_err());
        assert!(
            SdkEvalLoopConfig::new(vec!["mini".into()], 0)
                .validate()
                .is_err()
        );
        assert!(
            SdkEvalLoopConfig::new(vec!["".into()], 5)
                .validate()
                .is_err()
        );
    }

    #[test]
    fn config_builder() {
        let c = SdkEvalLoopConfig::new(vec!["mini".into()], 3)
            .with_levels(vec!["L1".into()])
            .with_output_dir("./out");
        assert_eq!(c.levels.unwrap().len(), 1);
        assert_eq!(c.output_dir, "./out");
    }

    #[test]
    fn all_sdks_constant() {
        assert_eq!(ALL_SDKS.len(), 5);
        assert!(ALL_SDKS.contains(&"mini"));
    }

    #[test]
    fn serde_roundtrips() {
        let iter = make_iter("mini", 0, 0.8);
        let j = serde_json::to_string(&iter).unwrap();
        assert_eq!(
            serde_json::from_str::<LoopIteration>(&j).unwrap().overall,
            0.8
        );

        let r = SdkEvalReport::new("mini");
        let j = serde_json::to_string(&r).unwrap();
        assert_eq!(
            serde_json::from_str::<SdkEvalReport>(&j).unwrap().sdk,
            "mini"
        );

        let f = FailureDetail {
            level: "L1".into(),
            failure_type: "lf".into(),
            error: Some("e".into()),
            score: Some(0.3),
        };
        let j = serde_json::to_string(&f).unwrap();
        assert_eq!(
            serde_json::from_str::<FailureDetail>(&j).unwrap().level,
            "L1"
        );
    }
}
