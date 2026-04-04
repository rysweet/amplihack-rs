//! Keyword search, contradiction detection, and query routing.

use std::collections::HashSet;

use super::{CONFIDENCE_SCORE_BOOST, HiveGraph};
use crate::models::HiveFact;

/// Tokenize a string into lowercase words, filtering single-char tokens.
pub fn tokenize(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(|w| w.to_lowercase())
        .collect()
}

/// Compute Jaccard word overlap between two strings.
pub fn word_overlap(a: &str, b: &str) -> f64 {
    let set_a = tokenize(a);
    let set_b = tokenize(b);
    let union_size = set_a.union(&set_b).count();
    if union_size == 0 {
        return 0.0;
    }
    set_a.intersection(&set_b).count() as f64 / union_size as f64
}

/// A fact scored by keyword relevance.
#[derive(Clone, Debug)]
pub struct ScoredFact {
    pub fact: HiveFact,
    pub score: f64,
}

impl HiveGraph {
    /// Keyword search: scores facts by word overlap with query.
    pub fn keyword_query(&self, query: &str, limit: usize) -> Vec<ScoredFact> {
        let keywords = tokenize(query);
        if keywords.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<ScoredFact> = self
            .facts
            .iter()
            .filter(|f| f.status != "retracted")
            .filter_map(|f| {
                let fact_words = tokenize(&f.content);
                let hits = keywords.intersection(&fact_words).count();
                if hits == 0 {
                    return None;
                }
                Some(ScoredFact {
                    fact: f.clone(),
                    score: hits as f64 + f.confidence * CONFIDENCE_SCORE_BOOST,
                })
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        scored
    }

    /// Detect facts that contradict given content for a concept.
    pub fn check_contradictions(&self, concept: &str, content: &str) -> Vec<HiveFact> {
        let concept_lower = concept.to_lowercase();
        self.facts
            .iter()
            .filter(|f| {
                f.status != "retracted"
                    && f.concept.to_lowercase() == concept_lower
                    && f.content != content
                    && word_overlap(&f.content, content) > 0.4
            })
            .cloned()
            .collect()
    }

    /// Route a query to expert agents based on domain keyword overlap.
    pub fn route_query(&self, query: &str) -> Vec<String> {
        let keywords = tokenize(query);
        if keywords.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(String, usize)> = self
            .agents
            .values()
            .filter(|a| a.status == "active")
            .map(|a| {
                let domain_words = tokenize(&a.domain);
                let overlap = keywords.intersection(&domain_words).count();
                (a.agent_id.clone(), overlap)
            })
            .filter(|(_, score)| *score > 0)
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().map(|(id, _)| id).collect()
    }
}
