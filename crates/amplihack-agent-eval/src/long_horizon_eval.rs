//! Long-horizon memory evaluation runner.
//!
//! Provides [`LongHorizonMemoryEval`], the native Rust equivalent of
//! Python's `amplihack.eval.long_horizon_memory.LongHorizonMemoryEval`.
//! Generates a deterministic multi-turn dialogue, poses recall questions,
//! grades answers, and produces a [`LongHorizonReport`].

use crate::error::EvalError;
use crate::grader::Grader;
use crate::long_horizon::{
    ALL_DIMENSIONS, DETERMINISTIC_DIMENSIONS, DimensionScore, EvalResult, GradingRubric,
    LongHorizonConfig, LongHorizonQuestion, LongHorizonReport, deterministic_grade,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Fact categories used in deterministic data generation
// ---------------------------------------------------------------------------

// Category names are embedded in the fact templates below.

// ---------------------------------------------------------------------------
// Deterministic fact generation
// ---------------------------------------------------------------------------

/// A fact injected into the dialogue during the learning phase.
#[derive(Debug, Clone)]
struct Fact {
    category: String,
    statement: String,
    keywords: Vec<String>,
}

/// Generate `count` deterministic facts, reproducible from a seed.
fn generate_facts(count: usize, seed: u64) -> Vec<Fact> {
    let mut facts = Vec::with_capacity(count);
    // Deterministic pseudo-random using a simple LCG seeded by `seed`.
    let mut rng_state = seed;

    let templates: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "people",
            "Engineer {} joined the team in sprint {}",
            vec!["engineer", "joined", "sprint"],
        ),
        (
            "events",
            "Incident {} occurred on day {} affecting {} services",
            vec!["incident", "occurred", "services"],
        ),
        (
            "technical",
            "Service {} uses {} replicas with {} MB memory",
            vec!["service", "replicas", "memory"],
        ),
        (
            "geography",
            "Data center {} is located in region {} with {} ms latency",
            vec!["data center", "region", "latency"],
        ),
        (
            "procedures",
            "Runbook step {}: execute {} then verify {}",
            vec!["runbook", "execute", "verify"],
        ),
    ];

    for i in 0..count {
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let template_idx = (rng_state >> 33) as usize % templates.len();
        let (cat, tmpl, kws) = &templates[template_idx];

        let a = (rng_state >> 16) as u32 % 1000;
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let b = (rng_state >> 16) as u32 % 500;
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let c = (rng_state >> 16) as u32 % 200;

        let statement = tmpl
            .replacen("{}", &a.to_string(), 1)
            .replacen("{}", &b.to_string(), 1)
            .replacen("{}", &c.to_string(), 1);

        facts.push(Fact {
            category: (*cat).to_string(),
            statement,
            keywords: kws.iter().map(|s| (*s).to_string()).collect(),
        });

        // Advance the seed per-iteration for reproducibility.
        rng_state = rng_state.wrapping_add(i as u64);
    }

    facts
}

/// Build questions from a subset of facts.
fn generate_questions(facts: &[Fact], num_questions: usize) -> Vec<LongHorizonQuestion> {
    let step = if facts.len() <= num_questions {
        1
    } else {
        facts.len() / num_questions
    };

    facts
        .iter()
        .step_by(step)
        .take(num_questions)
        .enumerate()
        .map(|(i, fact)| {
            let rubric = GradingRubric {
                required_keywords: fact.keywords.clone(),
                acceptable_paraphrases: vec![],
                incorrect_patterns: vec![],
                dimension_weights: HashMap::new(),
            };
            LongHorizonQuestion {
                question_id: format!("lh-q{i}"),
                text: format!("What do you know about: {}?", fact.statement),
                category: fact.category.clone(),
                expected_answer: fact.statement.clone(),
                rubric: Some(rubric),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Eval runner
// ---------------------------------------------------------------------------

/// Orchestrates a long-horizon memory evaluation.
///
/// Generates deterministic facts, builds questions with rubrics, grades
/// agent answers (or self-grades for pipeline validation), and produces a
/// [`LongHorizonReport`].
pub struct LongHorizonMemoryEval {
    config: LongHorizonConfig,
    grader: Box<dyn Grader>,
    output_dir: PathBuf,
    agent_name: String,
}

impl LongHorizonMemoryEval {
    pub fn new(
        config: LongHorizonConfig,
        grader: Box<dyn Grader>,
        output_dir: PathBuf,
        agent_name: impl Into<String>,
    ) -> Result<Self, EvalError> {
        config.validate()?;
        Ok(Self {
            config,
            grader,
            output_dir,
            agent_name: agent_name.into(),
        })
    }

    /// Run the full evaluation and return a report.
    ///
    /// Without an actual agent process, grades expected answers against
    /// themselves (self-grading) to validate the pipeline structure.
    pub fn run(&self) -> Result<LongHorizonReport, EvalError> {
        let start = Instant::now();

        // 1. Generate deterministic facts
        let facts = generate_facts(self.config.num_turns, self.config.seed);
        let learning_time = start.elapsed().as_secs_f64();

        // 2. Build questions
        let questions = generate_questions(&facts, self.config.num_questions);
        let questioning_start = Instant::now();

        // 3. Grade each question
        let mut results = Vec::with_capacity(questions.len());
        for q in &questions {
            // Self-grade: use expected answer as actual answer.
            let actual = &q.expected_answer;
            let result = self.grade_question(q, actual)?;
            results.push(result);
        }

        let questioning_time = questioning_start.elapsed().as_secs_f64();

        // 4. Build report
        let mut report = LongHorizonReport {
            num_turns: self.config.num_turns,
            num_questions: self.config.num_questions,
            total_facts_delivered: facts.len(),
            learning_time_s: learning_time,
            questioning_time_s: questioning_time,
            grading_time_s: start.elapsed().as_secs_f64(),
            overall_score: 0.0,
            category_breakdown: Vec::new(),
            results,
            memory_stats: HashMap::new(),
        };

        report.compute_breakdowns();

        // Write report to output_dir if writable.
        let _ = self.save_report(&report);

        Ok(report)
    }

    /// Grade a single question, combining deterministic and grader scores.
    fn grade_question(
        &self,
        question: &LongHorizonQuestion,
        actual: &str,
    ) -> Result<EvalResult, EvalError> {
        let mut dimensions = Vec::new();

        // Deterministic grading for applicable dimensions.
        if let Some(rubric) = &question.rubric {
            let det_scores = deterministic_grade(rubric, actual, DETERMINISTIC_DIMENSIONS);
            for (_, ds) in det_scores {
                dimensions.push(ds);
            }
        }

        // Use the configured grader for remaining dimensions.
        let grader_result = self.grader.grade(
            &question.text,
            &question.expected_answer,
            actual,
            crate::levels::TestLevel::L1Recall, // level used for grader heuristics
        )?;

        // Fill in any dimensions not yet graded.
        for &dim in ALL_DIMENSIONS {
            if !dimensions.iter().any(|d| d.dimension == dim) {
                dimensions.push(DimensionScore::new(
                    dim,
                    grader_result.score,
                    "grader-based",
                ));
            }
        }

        let overall = if dimensions.is_empty() {
            0.0
        } else {
            dimensions.iter().map(|d| d.score).sum::<f64>() / dimensions.len() as f64
        };

        Ok(EvalResult {
            question_id: question.question_id.clone(),
            question_text: question.text.clone(),
            category: question.category.clone(),
            expected_answer: question.expected_answer.clone(),
            actual_answer: actual.to_string(),
            dimensions,
            overall_score: overall,
            grading_time_s: 0.0,
        })
    }

    /// Best-effort save of the report JSON.
    fn save_report(&self, report: &LongHorizonReport) -> Result<(), EvalError> {
        std::fs::create_dir_all(&self.output_dir)?;
        let path = self
            .output_dir
            .join(format!("{}-long-horizon.json", self.agent_name));
        let json = serde_json::to_string_pretty(report)
            .map_err(|e| EvalError::harness(format!("JSON serialization: {e}")))?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Access the config.
    pub fn config(&self) -> &LongHorizonConfig {
        &self.config
    }

    /// Access the output dir.
    pub fn output_dir(&self) -> &PathBuf {
        &self.output_dir
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grader::SimpleGrader;

    #[test]
    fn generate_facts_deterministic() {
        let a = generate_facts(50, 42);
        let b = generate_facts(50, 42);
        assert_eq!(a.len(), b.len());
        for (fa, fb) in a.iter().zip(b.iter()) {
            assert_eq!(fa.statement, fb.statement);
        }
    }

    #[test]
    fn generate_facts_different_seeds() {
        let a = generate_facts(10, 42);
        let b = generate_facts(10, 99);
        // At least one fact should differ.
        assert!(
            a.iter()
                .zip(b.iter())
                .any(|(fa, fb)| fa.statement != fb.statement)
        );
    }

    #[test]
    fn generate_questions_respects_count() {
        let facts = generate_facts(100, 42);
        let questions = generate_questions(&facts, 10);
        assert_eq!(questions.len(), 10);
    }

    #[test]
    fn run_produces_report() {
        let dir = PathBuf::from(".");
        let config = LongHorizonConfig {
            num_turns: 20,
            num_questions: 5,
            grader_votes: 1,
            seed: 42,
            segment_size: None,
        };
        let grader = SimpleGrader::new(1).unwrap();
        let eval = LongHorizonMemoryEval::new(config, Box::new(grader), dir, "test-agent").unwrap();
        let report = eval.run().unwrap();
        assert_eq!(report.num_questions, 5);
        assert_eq!(report.results.len(), 5);
        assert!(
            report.overall_score > 0.0,
            "Self-grading should produce positive scores"
        );
        // Cleanup
        let _ = std::fs::remove_file("test-agent-long-horizon.json");
    }

    #[test]
    fn categories_are_populated() {
        let facts = generate_facts(100, 42);
        let categories: std::collections::HashSet<&str> =
            facts.iter().map(|f| f.category.as_str()).collect();
        assert!(categories.len() >= 3, "Should cover multiple categories");
    }

    #[test]
    fn report_breakdowns_computed() {
        let dir = PathBuf::from(".");
        let config = LongHorizonConfig {
            num_turns: 50,
            num_questions: 10,
            grader_votes: 1,
            seed: 7,
            segment_size: None,
        };
        let grader = SimpleGrader::new(1).unwrap();
        let eval = LongHorizonMemoryEval::new(config, Box::new(grader), dir, "test-bd").unwrap();
        let report = eval.run().unwrap();
        assert!(!report.category_breakdown.is_empty());
        for cb in &report.category_breakdown {
            assert!(cb.num_questions > 0);
        }
        let _ = std::fs::remove_file("test-bd-long-horizon.json");
    }
}
