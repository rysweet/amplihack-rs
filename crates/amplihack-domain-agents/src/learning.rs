use chrono::Utc;

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

    pub fn learn_from_content(&mut self, content: &str) -> Result<LearnedContent> {
        if self.learned_items.len() >= self.config.max_memory_items {
            // Evict oldest
            self.learned_items.remove(0);
        }

        let sentences: Vec<&str> = content
            .split(['.', '!', '?'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();

        let summary = sentences.first().unwrap_or(&content).to_string();

        let concepts: Vec<String> = content
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .map(|w| {
                w.to_lowercase()
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_string()
            })
            .filter(|w| !w.is_empty())
            .collect();

        let id = format!("lc-{}", self.learned_items.len() + 1);
        let learned = LearnedContent {
            content_id: id,
            summary,
            key_concepts: concepts,
            learned_at: Utc::now(),
        };

        self.learned_items.push(learned.clone());
        Ok(learned)
    }

    pub fn answer_question(&self, question: &str) -> Result<Answer> {
        let keywords: Vec<String> = question
            .to_lowercase()
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
            .filter(|w| !w.is_empty())
            .collect();

        let mut best: Option<&LearnedContent> = None;
        let mut best_score = 0usize;

        for item in &self.learned_items {
            let summary_lower = item.summary.to_lowercase();
            let score = keywords
                .iter()
                .filter(|kw| {
                    item.key_concepts.iter().any(|c| c.contains(kw.as_str()))
                        || summary_lower.contains(kw.as_str())
                })
                .count();
            if score > best_score {
                best_score = score;
                best = Some(item);
            }
        }

        if let Some(item) = best {
            Ok(Answer {
                content: item.summary.clone(),
                confidence: (best_score as f64 / keywords.len().max(1) as f64).min(1.0),
                sources: vec![item.content_id.clone()],
            })
        } else {
            Ok(Answer {
                content: "No relevant knowledge found".into(),
                confidence: 0.0,
                sources: vec![],
            })
        }
    }

    pub fn recall(&self, concept: &str) -> Result<Vec<LearnedContent>> {
        let lower = concept.to_lowercase();
        let matches: Vec<LearnedContent> = self
            .learned_items
            .iter()
            .filter(|item| {
                item.key_concepts.iter().any(|c| c.contains(&lower))
                    || item.summary.to_lowercase().contains(&lower)
            })
            .take(self.config.max_memory_items)
            .cloned()
            .collect();
        Ok(matches)
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
