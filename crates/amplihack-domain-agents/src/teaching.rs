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

    pub fn teach(&self, _content: &str) -> Result<TeachingResult> {
        todo!()
    }

    pub fn quiz(&self, _topic: &str, _num_questions: usize) -> Result<Vec<QuizQuestion>> {
        todo!()
    }

    pub fn evaluate_response(
        &self,
        _question: &QuizQuestion,
        _answer_index: usize,
    ) -> Result<EvaluationResult> {
        todo!()
    }

    pub fn evaluate_batch(
        &self,
        _questions: &[QuizQuestion],
        _answers: &[usize],
    ) -> Result<EvaluationResult> {
        todo!()
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
