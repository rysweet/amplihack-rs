//! Long-horizon memory stress test for agents.
//!
//! Ports Python `amplihack/eval/long_horizon_memory.py`:
//! - 1000-turn dialogue tests memory at scale
//! - Deterministic data generation, reproducible results
//! - Hybrid deterministic + LLM grading
//! - Multi-vote grading for stability
//! - Agent-agnostic interface

use crate::error::EvalError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Scoring dimensions
// ---------------------------------------------------------------------------

/// All scoring dimensions for long-horizon memory evaluation.
pub const ALL_DIMENSIONS: &[&str] = &[
    "factual_accuracy",
    "specificity",
    "temporal_awareness",
    "source_attribution",
    "confidence_calibration",
];

/// Dimensions that can be graded deterministically via keyword matching.
pub const DETERMINISTIC_DIMENSIONS: &[&str] = &["factual_accuracy", "specificity"];

/// Dimensions that always require LLM judgment.
pub const LLM_ONLY_DIMENSIONS: &[&str] =
    &["confidence_calibration", "source_attribution", "temporal_awareness"];

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Score on a single dimension for a single question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionScore {
    pub dimension: String,
    pub score: f64,
    #[serde(default)]
    pub reasoning: String,
}

impl DimensionScore {
    pub fn new(dimension: impl Into<String>, score: f64, reasoning: impl Into<String>) -> Self {
        Self {
            dimension: dimension.into(),
            score: score.clamp(0.0, 1.0),
            reasoning: reasoning.into(),
        }
    }
}

/// Result for a single question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub question_id: String,
    pub question_text: String,
    pub category: String,
    pub expected_answer: String,
    pub actual_answer: String,
    pub dimensions: Vec<DimensionScore>,
    pub overall_score: f64,
    #[serde(default)]
    pub grading_time_s: f64,
}

impl EvalResult {
    /// Compute overall score as the mean of dimension scores.
    pub fn compute_overall(&mut self) {
        if self.dimensions.is_empty() {
            self.overall_score = 0.0;
        } else {
            let sum: f64 = self.dimensions.iter().map(|d| d.score).sum();
            self.overall_score = sum / self.dimensions.len() as f64;
        }
    }
}

/// Aggregate scores for a question category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryBreakdown {
    pub category: String,
    pub num_questions: usize,
    pub avg_score: f64,
    pub min_score: f64,
    pub max_score: f64,
    #[serde(default)]
    pub dimension_averages: HashMap<String, f64>,
}

/// Complete evaluation report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongHorizonReport {
    pub num_turns: usize,
    pub num_questions: usize,
    pub total_facts_delivered: usize,
    pub learning_time_s: f64,
    pub questioning_time_s: f64,
    pub grading_time_s: f64,
    pub overall_score: f64,
    pub category_breakdown: Vec<CategoryBreakdown>,
    pub results: Vec<EvalResult>,
    #[serde(default)]
    pub memory_stats: HashMap<String, serde_json::Value>,
}

impl LongHorizonReport {
    /// Build category breakdowns from individual results.
    pub fn compute_breakdowns(&mut self) {
        let mut by_cat: HashMap<String, Vec<&EvalResult>> = HashMap::new();
        for r in &self.results {
            by_cat.entry(r.category.clone()).or_default().push(r);
        }

        self.category_breakdown = by_cat
            .into_iter()
            .map(|(cat, results)| {
                let scores: Vec<f64> = results.iter().map(|r| r.overall_score).collect();
                let n = scores.len();
                let avg = scores.iter().sum::<f64>() / n as f64;
                let min = scores.iter().cloned().fold(f64::INFINITY, f64::min);
                let max = scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

                // Per-dimension averages
                let mut dim_sums: HashMap<String, (f64, usize)> = HashMap::new();
                for r in &results {
                    for d in &r.dimensions {
                        let entry = dim_sums.entry(d.dimension.clone()).or_insert((0.0, 0));
                        entry.0 += d.score;
                        entry.1 += 1;
                    }
                }
                let dimension_averages: HashMap<String, f64> = dim_sums
                    .into_iter()
                    .map(|(k, (sum, cnt))| (k, sum / cnt as f64))
                    .collect();

                CategoryBreakdown {
                    category: cat,
                    num_questions: n,
                    avg_score: avg,
                    min_score: min,
                    max_score: max,
                    dimension_averages,
                }
            })
            .collect();

        // Overall score
        if !self.results.is_empty() {
            self.overall_score =
                self.results.iter().map(|r| r.overall_score).sum::<f64>()
                    / self.results.len() as f64;
        }
    }
}

// ---------------------------------------------------------------------------
// Grading rubric types
// ---------------------------------------------------------------------------

/// Grading rubric for deterministic scoring.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GradingRubric {
    #[serde(default)]
    pub required_keywords: Vec<String>,
    #[serde(default)]
    pub acceptable_paraphrases: Vec<String>,
    #[serde(default)]
    pub incorrect_patterns: Vec<String>,
    #[serde(default)]
    pub dimension_weights: HashMap<String, f64>,
}

/// A question with ground truth for the long-horizon eval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongHorizonQuestion {
    pub question_id: String,
    pub text: String,
    pub category: String,
    pub expected_answer: String,
    #[serde(default)]
    pub rubric: Option<GradingRubric>,
}

// ---------------------------------------------------------------------------
// Deterministic grading
// ---------------------------------------------------------------------------

/// Grade deterministic dimensions using keyword matching against a rubric.
///
/// Scoring: keyword match ratio + 0.25 bonus per acceptable paraphrase.
/// Incorrect patterns only zero the score when no correct keywords match.
pub fn deterministic_grade(
    rubric: &GradingRubric,
    actual_answer: &str,
    dimensions: &[&str],
) -> HashMap<String, DimensionScore> {
    let answer_lower = actual_answer.to_lowercase();
    let mut scores = HashMap::new();

    for &dim in dimensions {
        if !DETERMINISTIC_DIMENSIONS.contains(&dim) {
            continue;
        }

        // Keyword matching
        let matched = rubric
            .required_keywords
            .iter()
            .filter(|kw| answer_lower.contains(&kw.to_lowercase()))
            .count();

        let mut ratio = if rubric.required_keywords.is_empty() {
            0.5
        } else {
            matched as f64 / rubric.required_keywords.len() as f64
        };

        // Paraphrase bonus
        let paraphrase_hits = rubric
            .acceptable_paraphrases
            .iter()
            .filter(|p| answer_lower.contains(&p.to_lowercase()))
            .count();
        ratio = (ratio + paraphrase_hits as f64 * 0.25).min(1.0);

        // Check incorrect patterns
        let all_keywords_matched = !rubric.required_keywords.is_empty()
            && matched == rubric.required_keywords.len();
        let has_full_correct = all_keywords_matched || paraphrase_hits > 0;

        if !rubric.incorrect_patterns.is_empty() && !has_full_correct {
            let found_incorrect = rubric
                .incorrect_patterns
                .iter()
                .any(|pat| answer_lower.contains(&pat.to_lowercase()));
            if found_incorrect {
                scores.insert(
                    dim.to_string(),
                    DimensionScore::new(
                        dim,
                        0.0,
                        "Answer contains incorrect pattern without correct keywords",
                    ),
                );
                continue;
            }
        }

        let mut reasoning_parts = Vec::new();
        if !rubric.required_keywords.is_empty() {
            reasoning_parts.push(format!(
                "Matched {}/{} required keywords",
                matched,
                rubric.required_keywords.len()
            ));
        }
        if paraphrase_hits > 0 {
            reasoning_parts.push(format!("+{paraphrase_hits} paraphrase bonus"));
        }
        let reasoning = if reasoning_parts.is_empty() {
            "Deterministic score".to_string()
        } else {
            reasoning_parts.join("; ")
        };

        scores.insert(
            dim.to_string(),
            DimensionScore::new(dim, (ratio * 10000.0).round() / 10000.0, reasoning),
        );
    }

    scores
}

// ---------------------------------------------------------------------------
// Long-horizon eval configuration
// ---------------------------------------------------------------------------

/// Configuration for the long-horizon memory eval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongHorizonConfig {
    pub num_turns: usize,
    pub num_questions: usize,
    pub grader_votes: u8,
    pub seed: u64,
    #[serde(default)]
    pub segment_size: Option<usize>,
}

impl Default for LongHorizonConfig {
    fn default() -> Self {
        Self {
            num_turns: 100,
            num_questions: 20,
            grader_votes: 3,
            seed: 42,
            segment_size: None,
        }
    }
}

impl LongHorizonConfig {
    pub fn validate(&self) -> Result<(), EvalError> {
        if self.num_turns == 0 {
            return Err(EvalError::config("num_turns must be > 0"));
        }
        if self.num_questions == 0 {
            return Err(EvalError::config("num_questions must be > 0"));
        }
        if self.grader_votes == 0 {
            return Err(EvalError::config("grader_votes must be > 0"));
        }
        Ok(())
    }
}

/// Multi-vote grading: collect N grades and take the median per dimension.
pub fn multi_vote_grade(
    grades: Vec<Vec<DimensionScore>>,
    dimensions: &[&str],
) -> Vec<DimensionScore> {
    if grades.is_empty() {
        return dimensions
            .iter()
            .map(|d| DimensionScore::new(*d, 0.0, "No votes"))
            .collect();
    }
    if grades.len() == 1 {
        return grades.into_iter().next().unwrap();
    }

    let mut result = Vec::new();
    for &dim in dimensions {
        let mut vote_scores: Vec<f64> = Vec::new();
        let mut reasonings: Vec<String> = Vec::new();
        for grade_set in &grades {
            if let Some(ds) = grade_set.iter().find(|d| d.dimension == dim) {
                vote_scores.push(ds.score);
                reasonings.push(ds.reasoning.clone());
            }
        }

        if vote_scores.is_empty() {
            result.push(DimensionScore::new(dim, 0.0, "Not graded"));
            continue;
        }

        vote_scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = if vote_scores.len().is_multiple_of(2) {
            let mid = vote_scores.len() / 2;
            (vote_scores[mid - 1] + vote_scores[mid]) / 2.0
        } else {
            vote_scores[vote_scores.len() / 2]
        };

        // Pick reasoning closest to median
        let best_idx = vote_scores
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                ((**a) - median)
                    .abs()
                    .partial_cmp(&((**b) - median).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
            .unwrap_or(0);

        let reasoning = format!(
            "{} [median of {} votes]",
            reasonings.get(best_idx).cloned().unwrap_or_default(),
            vote_scores.len()
        );

        result.push(DimensionScore::new(dim, median, reasoning));
    }

    result
}

#[cfg(test)]
#[path = "tests/long_horizon_tests.rs"]
mod tests;
