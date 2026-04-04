//! Meta-eval teaching experiment runner.
//!
//! Ports Python `amplihack/eval/meta_eval_experiment.py`:
//! - Deterministic knowledge base about the eval system
//! - Quiz questions for student testing
//! - Experiment orchestration (build KB → teach → quiz → grade → report)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::metacognition_grader::MetacognitionGrader;

/// Knowledge base about the eval system — derived from actual source code.
pub fn eval_knowledge_base() -> Vec<String> {
    vec![
        "The evaluation harness tests agent learning using a learn-then-test pattern. \
         The agent first learns from news articles, then answers quiz questions \
         about what it learned. Learning and testing run in separate subprocesses \
         for memory isolation."
            .to_string(),
        "L1 (Recall) questions test direct fact retrieval from a single source. \
         They ask about specific entities, dates, or facts mentioned in articles."
            .to_string(),
        "L2 (Inference) questions test reasoning from facts. They require the agent \
         to connect cause and effect, or make predictions based on information."
            .to_string(),
        "L3 (Synthesis) questions require combining information from multiple sources. \
         They need at least 2 articles and ask about relationships or common themes."
            .to_string(),
        "L4 (Application) questions ask the agent to apply knowledge to new scenarios. \
         They test whether the agent can generalize from learned facts."
            .to_string(),
        "The grading system uses semantic comparison via LLM. \
         Scores range from 0.0 to 1.0: 1.0 = perfect, 0.8-0.9 = correct main points, \
         0.6-0.7 = partially correct, 0.0-0.3 = incorrect."
            .to_string(),
        "The quiz generator creates questions deterministically from articles. \
         It uses regex to extract entities and dates for L1 questions."
            .to_string(),
        "The harness runner orchestrates the full pipeline: \
         1. Collect news, 2. Generate quiz, 3. Learning phase, \
         4. Testing phase, 5. Grade answers."
            .to_string(),
        "To run the eval harness from command line: \
         python -m amplihack.eval.harness_runner --news-file <path> --output-dir ./eval_results"
            .to_string(),
        "The multi-source collector transforms WebSearch results into \
         structured NewsArticle objects with url, title, content, and published fields."
            .to_string(),
    ]
}

/// Quiz questions about the eval system.
pub fn eval_quiz() -> Vec<QuizQuestion> {
    vec![
        QuizQuestion {
            question: "What are the four cognitive levels (L1-L4) and what does each test?"
                .to_string(),
            expected_answer: "L1 (Recall) tests direct fact retrieval. L2 (Inference) tests \
                reasoning. L3 (Synthesis) requires combining multiple sources. L4 (Application) \
                tests applying knowledge to new scenarios."
                .to_string(),
        },
        QuizQuestion {
            question: "How does the grading system score answers?".to_string(),
            expected_answer: "Semantic LLM comparison with scores 0.0-1.0. \
                0.8-0.9 means correct main points with minor differences."
                .to_string(),
        },
        QuizQuestion {
            question: "What are the five steps in the evaluation pipeline?".to_string(),
            expected_answer: "1. Collect news, 2. Generate quiz, 3. Learning phase, \
                4. Testing phase, 5. Grade answers."
                .to_string(),
        },
        QuizQuestion {
            question: "Why does the harness use separate subprocesses?".to_string(),
            expected_answer: "For memory isolation — testing can only access \
                knowledge properly stored during learning."
                .to_string(),
        },
        QuizQuestion {
            question: "What format does the news input file need?".to_string(),
            expected_answer: "JSON with a 'sources' array containing objects with \
                url, title, content, and published fields."
                .to_string(),
        },
    ]
}

/// A quiz question with expected answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuizQuestion {
    pub question: String,
    pub expected_answer: String,
}

/// Configuration for the meta-eval experiment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentConfig {
    pub teaching_turns: usize,
    pub quiz_questions: usize,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_output_dir")]
    pub output_dir: String,
}

fn default_model() -> String {
    "claude-opus-4-6".to_string()
}

fn default_output_dir() -> String {
    "./meta_eval_results".to_string()
}

impl Default for ExperimentConfig {
    fn default() -> Self {
        Self {
            teaching_turns: 6,
            quiz_questions: 5,
            model: default_model(),
            output_dir: default_output_dir(),
        }
    }
}

/// Report from a complete meta-eval experiment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaExperimentReport {
    pub knowledge_base_size: usize,
    pub teaching_turns_completed: usize,
    pub quiz_results: Vec<QuizResult>,
    pub metacognition_scores: Vec<MetacognitionQuizScore>,
    pub overall_score: f64,
    pub summary: String,
}

/// Result of a single quiz question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuizResult {
    pub question: String,
    pub student_answer: String,
    pub self_explanation: String,
}

/// Metacognition score for a quiz question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetacognitionQuizScore {
    pub question: String,
    pub overall: f64,
    pub dimensions: HashMap<String, DimensionDetail>,
    pub summary: String,
}

/// Detail for a single dimension score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionDetail {
    pub score: f64,
    pub reasoning: String,
}

/// Meta-eval experiment orchestrator.
pub struct MetaEvalExperiment {
    pub config: ExperimentConfig,
}

impl MetaEvalExperiment {
    pub fn new(config: ExperimentConfig) -> Self {
        Self { config }
    }

    /// Build the deterministic knowledge base.
    pub fn build_knowledge_base(&self) -> Vec<String> {
        eval_knowledge_base()
    }

    /// Generate quiz questions (limited by config).
    pub fn generate_eval_quiz(&self) -> Vec<QuizQuestion> {
        let all = eval_quiz();
        all.into_iter()
            .take(self.config.quiz_questions)
            .collect()
    }

    /// Run the experiment with deterministic grading.
    ///
    /// Uses the metacognition grader for heuristic scoring.
    pub fn run(&self) -> MetaExperimentReport {
        let kb = self.build_knowledge_base();
        let quiz = self.generate_eval_quiz();

        // Simulate student answers from knowledge base
        let quiz_results: Vec<QuizResult> = quiz
            .iter()
            .map(|q| {
                // Find best-matching KB entry as "student answer"
                let best_match = kb
                    .iter()
                    .max_by_key(|fact| {
                        let q_words: Vec<&str> =
                            q.question.split_whitespace().collect();
                        q_words
                            .iter()
                            .filter(|w| {
                                fact.to_lowercase()
                                    .contains(&w.to_lowercase())
                            })
                            .count()
                    })
                    .cloned()
                    .unwrap_or_default();

                QuizResult {
                    question: q.question.clone(),
                    student_answer: best_match.clone(),
                    self_explanation: format!(
                        "I learned this because the teaching covered: {}",
                        best_match.chars().take(100).collect::<String>()
                    ),
                }
            })
            .collect();

        // Grade metacognition
        let grader = MetacognitionGrader::new(&self.config.model);
        let mut mc_scores = Vec::new();
        let mut score_values = Vec::new();

        for (q, result) in quiz.iter().zip(quiz_results.iter()) {
            let mc = grader.grade(
                &q.question,
                &q.expected_answer,
                &result.student_answer,
                &result.self_explanation,
            );

            let dimensions: HashMap<String, DimensionDetail> = mc
                .dimensions
                .iter()
                .map(|d| {
                    (
                        d.name.clone(),
                        DimensionDetail {
                            score: d.score,
                            reasoning: d.reasoning.clone(),
                        },
                    )
                })
                .collect();

            mc_scores.push(MetacognitionQuizScore {
                question: q.question.clone(),
                overall: mc.overall_score,
                dimensions,
                summary: mc.summary,
            });
            score_values.push(mc.overall_score);
        }

        let overall = if score_values.is_empty() {
            0.0
        } else {
            score_values.iter().sum::<f64>() / score_values.len() as f64
        };

        let quality = if overall >= 0.8 {
            "excellent"
        } else if overall >= 0.6 {
            "good"
        } else if overall >= 0.4 {
            "moderate"
        } else {
            "limited"
        };

        let summary = format!(
            "Meta-eval experiment completed with {quality} results. \
             Teaching: {} turns. Quiz: {} questions. \
             Overall metacognition score: {overall:.2}.",
            self.config.teaching_turns,
            quiz_results.len()
        );

        MetaExperimentReport {
            knowledge_base_size: kb.len(),
            teaching_turns_completed: self.config.teaching_turns,
            quiz_results,
            metacognition_scores: mc_scores,
            overall_score: overall,
            summary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knowledge_base_non_empty() {
        let kb = eval_knowledge_base();
        assert_eq!(kb.len(), 10);
        assert!(kb[0].contains("harness"));
    }

    #[test]
    fn quiz_has_5_questions() {
        let quiz = eval_quiz();
        assert_eq!(quiz.len(), 5);
    }

    #[test]
    fn config_default() {
        let cfg = ExperimentConfig::default();
        assert_eq!(cfg.teaching_turns, 6);
        assert_eq!(cfg.quiz_questions, 5);
    }

    #[test]
    fn experiment_runs() {
        let exp = MetaEvalExperiment::new(ExperimentConfig {
            quiz_questions: 2,
            ..Default::default()
        });
        let report = exp.run();

        assert_eq!(report.knowledge_base_size, 10);
        assert_eq!(report.quiz_results.len(), 2);
        assert_eq!(report.metacognition_scores.len(), 2);
        assert!(report.overall_score >= 0.0);
        assert!(!report.summary.is_empty());
    }

    #[test]
    fn experiment_zero_questions() {
        let exp = MetaEvalExperiment::new(ExperimentConfig {
            quiz_questions: 0,
            ..Default::default()
        });
        let report = exp.run();
        assert_eq!(report.quiz_results.len(), 0);
        assert!((report.overall_score).abs() < f64::EPSILON);
    }

    #[test]
    fn report_serde_roundtrip() {
        let exp = MetaEvalExperiment::new(ExperimentConfig {
            quiz_questions: 1,
            ..Default::default()
        });
        let report = exp.run();
        let json = serde_json::to_string(&report).unwrap();
        let back: MetaExperimentReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.knowledge_base_size, report.knowledge_base_size);
    }
}
