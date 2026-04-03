use crate::error::Result;
use crate::models::{Answer, LearnedContent, LearningConfig};

pub struct LearningAgent {
    config: LearningConfig,
    learned_items: Vec<LearnedContent>,
}

impl LearningAgent {
    pub fn new(config: LearningConfig) -> Self {
        Self {
            config,
            learned_items: Vec::new(),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(LearningConfig::default())
    }

    pub fn config(&self) -> &LearningConfig {
        &self.config
    }

    pub fn learned_count(&self) -> usize {
        self.learned_items.len()
    }

    pub fn learn_from_content(&mut self, _content: &str) -> Result<LearnedContent> {
        todo!()
    }

    pub fn answer_question(&self, _question: &str) -> Result<Answer> {
        todo!()
    }

    pub fn recall(&self, _concept: &str) -> Result<Vec<LearnedContent>> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_with_config() {
        let config = LearningConfig {
            retention_strategy: "leitner".into(),
            max_memory_items: 500,
        };
        let agent = LearningAgent::new(config.clone());
        assert_eq!(agent.config(), &config);
        assert_eq!(agent.learned_count(), 0);
    }

    #[test]
    fn with_defaults() {
        let agent = LearningAgent::with_defaults();
        assert_eq!(agent.config().retention_strategy, "spaced_repetition");
        assert_eq!(agent.config().max_memory_items, 1000);
        assert_eq!(agent.learned_count(), 0);
    }
}
