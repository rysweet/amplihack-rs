//! Multi-seed holdout evaluation for long-horizon memory.
//!
//! Ports Python `amplihack/eval/long_horizon_multi_seed.py`:
//! - Multi-seed evaluation across random seeds
//! - Per-question variance analysis and noisy question detection
//! - Per-category statistics (mean ± stddev)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Default random seeds for multi-seed evaluation.
pub const DEFAULT_SEEDS: [u64; 4] = [42, 123, 456, 789];

/// Threshold in percentage points for flagging noisy questions.
pub const NOISY_THRESHOLD_PP: f64 = 10.0;

/// Per-question variance across seeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionVariance {
    pub question_id: String,
    pub question_text: String,
    pub category: String,
    pub scores_by_seed: HashMap<u64, f64>,
    pub mean_score: f64,
    pub stddev: f64,
    pub is_noisy: bool,
}

/// Per-category statistics across seeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryStats {
    pub category: String,
    pub mean_score: f64,
    pub stddev: f64,
    pub min_score: f64,
    pub max_score: f64,
    pub scores_by_seed: HashMap<u64, f64>,
}

/// Per-seed evaluation result (summary).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedResult {
    pub seed: u64,
    pub overall_score: f64,
    pub question_scores: Vec<QuestionScore>,
}

/// Score for a single question in a seed run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionScore {
    pub question_id: String,
    pub question_text: String,
    pub category: String,
    pub overall_score: f64,
}

/// Aggregate report across multiple seeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiSeedReport {
    pub seeds: Vec<u64>,
    pub num_turns: usize,
    pub num_questions: usize,
    pub total_time_s: f64,
    pub overall_mean: f64,
    pub overall_stddev: f64,
    pub category_stats: Vec<CategoryStats>,
    pub noisy_questions: Vec<QuestionVariance>,
    pub all_question_variances: Vec<QuestionVariance>,
    pub num_noisy_questions: usize,
    pub total_questions_evaluated: usize,
}

/// Compute sample standard deviation, returning 0 for <2 values.
pub fn safe_stddev(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance =
        values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (values.len() - 1) as f64;
    variance.sqrt()
}

/// Build a multi-seed report from individual seed results.
pub fn build_multi_seed_report(
    seed_results: &[SeedResult],
    num_turns: usize,
    num_questions: usize,
    total_time_s: f64,
) -> MultiSeedReport {
    let seeds: Vec<u64> = seed_results.iter().map(|r| r.seed).collect();

    // Overall mean/stddev
    let overall_scores: Vec<f64> = seed_results.iter().map(|r| r.overall_score).collect();
    let overall_mean = if overall_scores.is_empty() {
        0.0
    } else {
        overall_scores.iter().sum::<f64>() / overall_scores.len() as f64
    };
    let overall_stddev = safe_stddev(&overall_scores);

    // Collect per-question scores across seeds
    let mut question_scores: HashMap<String, HashMap<u64, f64>> = HashMap::new();
    let mut question_meta: HashMap<String, (String, String)> = HashMap::new();

    for result in seed_results {
        for qs in &result.question_scores {
            question_scores
                .entry(qs.question_id.clone())
                .or_default()
                .insert(result.seed, qs.overall_score);
            question_meta
                .entry(qs.question_id.clone())
                .or_insert_with(|| (qs.question_text.clone(), qs.category.clone()));
        }
    }

    // Compute per-question variance
    let mut all_variances = Vec::with_capacity(question_scores.len());
    let mut noisy_indices = Vec::new();

    for (qid, seed_scores) in &question_scores {
        let (text, cat) = question_meta.get(qid).cloned().unwrap_or_default();
        let values: Vec<f64> = seed_scores.values().copied().collect();
        let mean_val = if values.is_empty() {
            0.0
        } else {
            values.iter().sum::<f64>() / values.len() as f64
        };
        let std_val = safe_stddev(&values);
        let is_noisy = std_val > (NOISY_THRESHOLD_PP / 100.0);

        if is_noisy {
            noisy_indices.push(all_variances.len());
        }
        all_variances.push(QuestionVariance {
            question_id: qid.clone(),
            question_text: text,
            category: cat,
            scores_by_seed: seed_scores.clone(),
            mean_score: mean_val,
            stddev: std_val,
            is_noisy,
        });
    }

    // Build noisy list by cloning only flagged entries (avoids cloning every entry)
    let noisy: Vec<QuestionVariance> = noisy_indices
        .iter()
        .map(|&i| all_variances[i].clone())
        .collect();

    // Per-category stats
    let mut categories: HashMap<String, HashMap<u64, Vec<f64>>> = HashMap::new();
    for result in seed_results {
        for qs in &result.question_scores {
            categories
                .entry(qs.category.clone())
                .or_default()
                .entry(result.seed)
                .or_default()
                .push(qs.overall_score);
        }
    }

    let mut category_stats: Vec<CategoryStats> = categories
        .into_iter()
        .map(|(cat, seed_map)| {
            let scores_by_seed: HashMap<u64, f64> = seed_map
                .into_iter()
                .map(|(seed, scores)| {
                    let avg = scores.iter().sum::<f64>() / scores.len().max(1) as f64;
                    (seed, avg)
                })
                .collect();
            let values: Vec<f64> = scores_by_seed.values().copied().collect();
            let mean = if values.is_empty() {
                0.0
            } else {
                values.iter().sum::<f64>() / values.len() as f64
            };

            CategoryStats {
                category: cat,
                mean_score: mean,
                stddev: safe_stddev(&values),
                min_score: values.iter().cloned().fold(f64::INFINITY, f64::min),
                max_score: values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                scores_by_seed,
            }
        })
        .collect();
    category_stats.sort_by(|a, b| a.category.cmp(&b.category));

    let num_noisy = noisy.len();
    let total_evaluated = all_variances.len();

    MultiSeedReport {
        seeds,
        num_turns,
        num_questions,
        total_time_s,
        overall_mean,
        overall_stddev,
        category_stats,
        noisy_questions: noisy,
        all_question_variances: all_variances,
        num_noisy_questions: num_noisy,
        total_questions_evaluated: total_evaluated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_stddev_empty() {
        assert!((safe_stddev(&[]) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn safe_stddev_single() {
        assert!((safe_stddev(&[5.0]) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn safe_stddev_known() {
        // stddev of [2, 4, 4, 4, 5, 5, 7, 9] = 2.138...
        let vals = vec![2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let sd = safe_stddev(&vals);
        assert!((sd - 2.138).abs() < 0.01);
    }

    #[test]
    fn default_seeds() {
        assert_eq!(DEFAULT_SEEDS.len(), 4);
        assert_eq!(DEFAULT_SEEDS[0], 42);
    }

    #[test]
    fn build_report_empty() {
        let report = build_multi_seed_report(&[], 100, 20, 10.0);
        assert!(report.seeds.is_empty());
        assert!((report.overall_mean).abs() < f64::EPSILON);
    }

    #[test]
    fn build_report_single_seed() {
        let results = vec![SeedResult {
            seed: 42,
            overall_score: 0.75,
            question_scores: vec![QuestionScore {
                question_id: "q1".to_string(),
                question_text: "Test question".to_string(),
                category: "recall".to_string(),
                overall_score: 0.75,
            }],
        }];
        let report = build_multi_seed_report(&results, 100, 1, 5.0);
        assert_eq!(report.seeds, vec![42]);
        assert!((report.overall_mean - 0.75).abs() < f64::EPSILON);
        assert_eq!(report.total_questions_evaluated, 1);
    }

    #[test]
    fn noisy_detection() {
        let results = vec![
            SeedResult {
                seed: 42,
                overall_score: 0.8,
                question_scores: vec![QuestionScore {
                    question_id: "q1".to_string(),
                    question_text: "Noisy Q".to_string(),
                    category: "recall".to_string(),
                    overall_score: 0.9,
                }],
            },
            SeedResult {
                seed: 123,
                overall_score: 0.5,
                question_scores: vec![QuestionScore {
                    question_id: "q1".to_string(),
                    question_text: "Noisy Q".to_string(),
                    category: "recall".to_string(),
                    overall_score: 0.3,
                }],
            },
        ];
        let report = build_multi_seed_report(&results, 100, 1, 5.0);
        // stddev of [0.9, 0.3] ≈ 0.424 > 0.10 threshold
        assert_eq!(report.num_noisy_questions, 1);
        assert!(report.noisy_questions[0].is_noisy);
    }

    #[test]
    fn category_stats_computed() {
        let results = vec![
            SeedResult {
                seed: 42,
                overall_score: 0.8,
                question_scores: vec![
                    QuestionScore {
                        question_id: "q1".to_string(),
                        question_text: "Q1".to_string(),
                        category: "recall".to_string(),
                        overall_score: 0.9,
                    },
                    QuestionScore {
                        question_id: "q2".to_string(),
                        question_text: "Q2".to_string(),
                        category: "inference".to_string(),
                        overall_score: 0.7,
                    },
                ],
            },
            SeedResult {
                seed: 123,
                overall_score: 0.7,
                question_scores: vec![
                    QuestionScore {
                        question_id: "q1".to_string(),
                        question_text: "Q1".to_string(),
                        category: "recall".to_string(),
                        overall_score: 0.8,
                    },
                    QuestionScore {
                        question_id: "q2".to_string(),
                        question_text: "Q2".to_string(),
                        category: "inference".to_string(),
                        overall_score: 0.6,
                    },
                ],
            },
        ];
        let report = build_multi_seed_report(&results, 100, 2, 10.0);
        assert_eq!(report.category_stats.len(), 2);
    }

    #[test]
    fn report_serde_roundtrip() {
        let report = build_multi_seed_report(&[], 50, 10, 2.5);
        let json = serde_json::to_string(&report).unwrap();
        let back: MultiSeedReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.num_turns, 50);
    }
}
