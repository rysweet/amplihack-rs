use crate::error::Result;
use crate::models::{EvaluationResult, QuizQuestion, TeachingConfig, TeachingResult};

pub struct TeachingAgent {
    config: TeachingConfig,
}

impl TeachingAgent {
    pub fn new(config: TeachingConfig) -> Self {
        Self { config }
    }

    pub fn with_defaults() -> Self {
        Self::new(TeachingConfig::default())
    }

    pub fn config(&self) -> &TeachingConfig {
        &self.config
    }

    pub fn teach(&self, content: &str) -> Result<TeachingResult> {
        let key_points: Vec<String> = content
            .split(['.', '!', '?'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();

        Ok(TeachingResult {
            content_delivered: content.to_string(),
            topics_covered: key_points,
        })
    }

    pub fn quiz(&self, topic: &str, num_questions: usize) -> Result<Vec<QuizQuestion>> {
        if topic.trim().is_empty() {
            return Err(crate::error::DomainError::InvalidInput(
                "topic must not be empty".into(),
            ));
        }
        if num_questions == 0 {
            return Err(crate::error::DomainError::InvalidInput(
                "num_questions must be > 0".into(),
            ));
        }
        let questions: Vec<QuizQuestion> = (0..num_questions)
            .map(|i| QuizQuestion {
                question: format!("Question {} about {}", i + 1, topic),
                options: vec![
                    format!("Correct answer for Q{}", i + 1),
                    format!("Wrong answer A for Q{}", i + 1),
                    format!("Wrong answer B for Q{}", i + 1),
                    format!("Wrong answer C for Q{}", i + 1),
                ],
                correct_index: 0,
            })
            .collect();
        Ok(questions)
    }

    pub fn evaluate_response(
        &self,
        question: &QuizQuestion,
        answer_index: usize,
    ) -> Result<EvaluationResult> {
        if question.correct_index >= question.options.len() {
            return Err(crate::error::DomainError::InvalidInput(format!(
                "correct_index {} out of bounds for {} options",
                question.correct_index,
                question.options.len()
            )));
        }
        if answer_index >= question.options.len() {
            return Err(crate::error::DomainError::InvalidInput(format!(
                "answer_index {} out of bounds for {} options",
                answer_index,
                question.options.len()
            )));
        }
        let correct = answer_index == question.correct_index;
        Ok(EvaluationResult {
            score: if correct { 1.0 } else { 0.0 },
            feedback: if correct {
                "Correct!".into()
            } else {
                format!(
                    "Incorrect. The correct answer was option {}",
                    question.correct_index
                )
            },
            correct_count: usize::from(correct),
            total_count: 1,
        })
    }

    pub fn evaluate_batch(
        &self,
        questions: &[QuizQuestion],
        answers: &[usize],
    ) -> Result<EvaluationResult> {
        if questions.len() != answers.len() {
            return Err(crate::error::DomainError::InvalidInput(format!(
                "questions/answers length mismatch: {} vs {}",
                questions.len(),
                answers.len()
            )));
        }
        for (i, q) in questions.iter().enumerate() {
            if q.correct_index >= q.options.len() {
                return Err(crate::error::DomainError::InvalidInput(format!(
                    "question {}: correct_index {} out of bounds for {} options",
                    i,
                    q.correct_index,
                    q.options.len()
                )));
            }
        }
        let total = questions.len();
        let correct_count = questions
            .iter()
            .zip(answers.iter())
            .filter(|&(q, a)| *a == q.correct_index)
            .count();
        let score = if total == 0 {
            0.0
        } else {
            correct_count as f64 / total as f64
        };
        Ok(EvaluationResult {
            score,
            feedback: format!("{correct_count}/{total} correct"),
            correct_count,
            total_count: total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_config() {
        let config = TeachingConfig {
            max_quiz_questions: 5,
            difficulty_level: "hard".into(),
            subject_area: "math".into(),
        };
        let agent = TeachingAgent::new(config.clone());
        assert_eq!(agent.config(), &config);
    }

    #[test]
    fn with_defaults() {
        let agent = TeachingAgent::with_defaults();
        assert_eq!(agent.config().max_quiz_questions, 10);
        assert_eq!(agent.config().difficulty_level, "medium");
    }
}
