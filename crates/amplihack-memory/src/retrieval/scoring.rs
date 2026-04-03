//! N-gram overlap scoring, deduplication, and fact merging.
//!
//! Ported from the scoring helpers in Python `retrieval_strategies.py`.

use super::constants::{BIGRAM_WEIGHT, UNIGRAM_WEIGHT};
use super::types::Fact;
use std::collections::HashSet;

/// Compute a weighted unigram + bigram overlap score between `query` and `text`.
///
/// Returns a value in `[0.0, 1.0]`.
pub fn ngram_overlap_score(query: &str, text: &str) -> f64 {
    let q_tokens: Vec<&str> = query.split_whitespace().collect();
    let t_tokens: Vec<&str> = text.split_whitespace().collect();

    if q_tokens.is_empty() || t_tokens.is_empty() {
        return 0.0;
    }

    // Unigrams
    let q_unigrams: HashSet<String> = q_tokens.iter().map(|w| w.to_lowercase()).collect();
    let t_unigrams: HashSet<String> = t_tokens.iter().map(|w| w.to_lowercase()).collect();
    let unigram_overlap = q_unigrams.intersection(&t_unigrams).count() as f64;
    let unigram_score = if q_unigrams.is_empty() {
        0.0
    } else {
        unigram_overlap / q_unigrams.len() as f64
    };

    // Bigrams
    let q_bigrams: HashSet<String> = q_tokens
        .windows(2)
        .map(|w| format!("{} {}", w[0].to_lowercase(), w[1].to_lowercase()))
        .collect();
    let t_bigrams: HashSet<String> = t_tokens
        .windows(2)
        .map(|w| format!("{} {}", w[0].to_lowercase(), w[1].to_lowercase()))
        .collect();
    let bigram_score = if q_bigrams.is_empty() {
        0.0
    } else {
        let bigram_overlap = q_bigrams.intersection(&t_bigrams).count() as f64;
        bigram_overlap / q_bigrams.len() as f64
    };

    UNIGRAM_WEIGHT * unigram_score + BIGRAM_WEIGHT * bigram_score
}

/// Deduplicate facts by their `dedup_key()`.
pub fn deduplicate_facts(facts: Vec<Fact>) -> Vec<Fact> {
    let mut seen = HashSet::new();
    facts
        .into_iter()
        .filter(|f| seen.insert(f.dedup_key()))
        .collect()
}

/// Merge two fact lists, deduplicating by `dedup_key()`.
///
/// Facts from `base` are kept first; `additional` facts are appended if unseen.
pub fn merge_facts(base: Vec<Fact>, additional: Vec<Fact>) -> Vec<Fact> {
    let mut seen: HashSet<String> = base.iter().map(|f| f.dedup_key()).collect();
    let mut merged = base;
    for fact in additional {
        let key = fact.dedup_key();
        if seen.insert(key) {
            merged.push(fact);
        }
    }
    merged
}

/// Check whether the text of a fact mentions any of the given entity IDs.
pub fn fact_mentions_entity(fact: &Fact, entity_ids: &HashSet<String>) -> bool {
    let text = format!("{} {}", fact.context, fact.outcome).to_lowercase();
    entity_ids.iter().any(|id| text.contains(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ngram_overlap_identical() {
        let score = ngram_overlap_score("hello world", "hello world");
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ngram_overlap_no_match() {
        let score = ngram_overlap_score("foo bar", "baz qux");
        assert!(score.abs() < f64::EPSILON);
    }

    #[test]
    fn ngram_overlap_partial() {
        let score = ngram_overlap_score("hello world", "hello earth");
        assert!(score > 0.0);
        assert!(score < 1.0);
    }

    #[test]
    fn ngram_overlap_empty() {
        assert!(ngram_overlap_score("", "hello").abs() < f64::EPSILON);
        assert!(ngram_overlap_score("hello", "").abs() < f64::EPSILON);
    }

    #[test]
    fn deduplicate_removes_exact_dupes() {
        let f1 = Fact::new("ctx", "out1");
        let f2 = Fact::new("ctx", "out1");
        let f3 = Fact::new("ctx", "out2");
        let result = deduplicate_facts(vec![f1, f2, f3]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn merge_deduplicates() {
        let a = vec![Fact::new("ctx", "out1")];
        let b = vec![Fact::new("ctx", "out1"), Fact::new("ctx", "out2")];
        let merged = merge_facts(a, b);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn fact_mentions_entity_match() {
        let f = Fact::new("incident report", "INC-2024-001 was critical");
        let ids: HashSet<String> = ["inc-2024-001".into()].into_iter().collect();
        assert!(fact_mentions_entity(&f, &ids));
    }

    #[test]
    fn fact_mentions_entity_no_match() {
        let f = Fact::new("general info", "nothing here");
        let ids: HashSet<String> = ["inc-2024-001".into()].into_iter().collect();
        assert!(!fact_mentions_entity(&f, &ids));
    }
}
