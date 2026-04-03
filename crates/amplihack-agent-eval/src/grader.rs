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

impl Grader for SimpleGrader {
    fn grade(
        &self,
        _question: &str,
        _expected: &str,
        _actual: &str,
        _level: TestLevel,
    ) -> Result<GradeResult, EvalError> {
        todo!("SimpleGrader::grade not yet implemented")
    }
}

/// Multi-vote grader that averages across multiple grading passes.
pub fn grade_with_votes(
    _grader: &dyn Grader,
    _question: &str,
    _expected: &str,
    _actual: &str,
    _level: TestLevel,
    _votes: u8,
) -> Result<GradeResult, EvalError> {
    todo!("grade_with_votes not yet implemented")
}
