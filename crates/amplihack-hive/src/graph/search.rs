use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::models::HiveFact;

use super::{HiveGraph, CONFIDENCE_SCORE_BOOST};

/// Tokenize text into lowercase words longer than 1 character.
pub fn tokenize(text: &str) -> HashSet<String> {
    text.split_whitespace()
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() > 1)
        .collect()
}

/// Compute Jaccard similarity based on word overlap between two strings.
pub fn word_overlap(a: &str, b: &str) -> f64 {
    let set_a = tokenize(a);
    let set_b = tokenize(b);
    if set_a.is_empty() && set_b.is_empty() {
        return 0.0;
    }
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

/// A fact with a search relevance score.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoredFact {
    pub fact: HiveFact,
    pub score: f64,
}

impl HiveGraph {
    /// Keyword-based query: score = token_hits + confidence * CONFIDENCE_SCORE_BOOST.
    /// Excludes retracted facts.
    pub fn keyword_query(&self, query: &str, limit: usize) -> Vec<ScoredFact> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<ScoredFact> = self
            .facts
            .iter()
            .filter(|f| f.status != "retracted")
            .filter_map(|f| {
                let fact_tokens = tokenize(&format!("{} {}", f.concept, f.content));
                let hits = query_tokens
                    .iter()
                    .filter(|qt| fact_tokens.contains(*qt))
                    .count();
                if hits == 0 {
                    return None;
                }
                let score = hits as f64 + f.confidence * CONFIDENCE_SCORE_BOOST;
                Some(ScoredFact {
                    fact: f.clone(),
                    score,
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

    /// Find facts that may contradict the given concept+content.
    /// Same concept (case-insensitive exact match), word_overlap > 0.4,
    /// different content, excludes retracted.
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

    /// Route a query to agents whose domain has the highest keyword overlap.
    /// Returns agent IDs sorted by overlap descending, active agents only.
    pub fn route_query(&self, query: &str) -> Vec<String> {
        let mut agent_scores: Vec<(String, f64)> = self
            .agents
            .values()
            .filter(|a| a.status == "active")
            .map(|a| {
                let overlap = word_overlap(query, &a.domain);
                (a.agent_id.clone(), overlap)
            })
            .collect();
        agent_scores.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        agent_scores.into_iter().map(|(id, _)| id).collect()
    }
}
