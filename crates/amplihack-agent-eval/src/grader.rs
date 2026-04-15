//! Grading infrastructure for evaluating agent answers.

use crate::error::EvalError;
use crate::levels::TestLevel;
use crate::models::GradeResult;

/// Trait for grading agent responses.
pub trait Grader: Send + Sync {
    /// Grade an answer against an expected result.
    fn grade(
        &self,
        question: &str,
        expected: &str,
        actual: &str,
        level: TestLevel,
    ) -> Result<GradeResult, EvalError>;
}

/// Simple grader using string similarity and level-specific heuristics.
#[derive(Debug, Clone)]
pub struct SimpleGrader {
    /// Number of independent votes to average.
    pub votes: u8,
}

impl SimpleGrader {
    pub fn new(votes: u8) -> Result<Self, EvalError> {
        if votes == 0 {
            return Err(EvalError::config("votes must be > 0"));
        }
        Ok(Self { votes })
    }
}

impl SimpleGrader {
    /// Single-pass grading logic (no vote aggregation).
    fn grade_single(
        &self,
        expected: &str,
        actual: &str,
        level: TestLevel,
    ) -> Result<GradeResult, EvalError> {
        if actual.is_empty() {
            return GradeResult::new(0.0, "Empty answer — no content to grade");
        }

        let expected_lower = expected.to_lowercase();
        let actual_lower = actual.to_lowercase();

        if expected_lower == actual_lower {
            return GradeResult::new(1.0, "Exact match");
        }

        let contains_expected = actual_lower.contains(&expected_lower);

        let expected_words: std::collections::HashSet<&str> =
            expected_lower.split_whitespace().collect();
        let actual_words: std::collections::HashSet<&str> =
            actual_lower.split_whitespace().collect();

        let overlap = expected_words.intersection(&actual_words).count();
        let word_score = if expected_words.is_empty() {
            0.0
        } else {
            overlap as f64 / expected_words.len() as f64
        };

        let level_bonus = match level {
            TestLevel::L3TemporalReasoning => {
                if has_temporal_ordering(&actual_lower, &expected_lower) {
                    0.1
                } else {
                    -0.1
                }
            }
            TestLevel::L5ContradictionHandling
                if actual_lower.contains("contradict")
                    || actual_lower.contains("outdated")
                    || actual_lower.contains("incorrect") =>
            {
                0.1
            }
            TestLevel::L5ContradictionHandling => 0.0,
            _ => 0.0,
        };

        let mut score = if contains_expected {
            0.9
        } else {
            word_score * 0.8
        };
        score = (score + level_bonus).clamp(0.0, 1.0);

        let reasoning = format!(
            "Word overlap: {overlap}/{}, containment: {contains_expected}, level: {}",
            expected_words.len(),
            level.display_name()
        );
        GradeResult::new(score, reasoning)
    }

    /// Multi-vote grading: run `grade_single` N times and take the median.
    ///
    /// For deterministic graders like `SimpleGrader`, every call to `grade_single`
    /// returns the same result, so multiple votes are redundant. We short-circuit
    /// by calling `grade_single` once and replicating its score across all votes.
    fn grade_multi_vote(
        &self,
        expected: &str,
        actual: &str,
        level: TestLevel,
    ) -> Result<GradeResult, EvalError> {
        let vote_count = self.votes.clamp(1, 9) as usize;

        // Deterministic grader: a single call suffices since every invocation
        // produces the identical score.
        let single = self.grade_single(expected, actual, level)?;
        let scores = vec![single.score; vote_count];

        let mut result = GradeResult::new(single.score, single.reasoning)?;
        result = result.with_votes(scores);
        Ok(result)
    }
}

impl Grader for SimpleGrader {
    fn grade(
        &self,
        _question: &str,
        expected: &str,
        actual: &str,
        level: TestLevel,
    ) -> Result<GradeResult, EvalError> {
        if self.votes > 1 {
            self.grade_multi_vote(expected, actual, level)
        } else {
            self.grade_single(expected, actual, level)
        }
    }
}

/// Check if actual has temporal ordering consistent with expected.
fn has_temporal_ordering(actual: &str, expected: &str) -> bool {
    let temporal_words = ["before", "after", "first", "then", "later", "earlier"];
    temporal_words
        .iter()
        .any(|w| actual.contains(w) && expected.contains(w))
}

/// Multi-vote grader that averages across multiple grading passes.
pub fn grade_with_votes(
    grader: &dyn Grader,
    question: &str,
    expected: &str,
    actual: &str,
    level: TestLevel,
    votes: u8,
) -> Result<GradeResult, EvalError> {
    let vote_count = votes.clamp(1, 9);
    let mut scores = Vec::with_capacity(vote_count as usize);
    let mut last_reasoning = String::new();

    for _ in 0..vote_count {
        let result = grader.grade(question, expected, actual, level)?;
        scores.push(result.score);
        last_reasoning = result.reasoning;
    }

    // Use median score
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median_score = if scores.len() % 2 == 0 {
        (scores[scores.len() / 2 - 1] + scores[scores.len() / 2]) / 2.0
    } else {
        scores[scores.len() / 2]
    };

    let mut result = GradeResult::new(median_score, last_reasoning)?;
    result = result.with_votes(scores);
    Ok(result)
}
