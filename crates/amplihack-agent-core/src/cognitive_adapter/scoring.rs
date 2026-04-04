//! Text scoring, stop-word filtering, and hive conversion helpers.
//!
//! Ports `_filter_stop_words`, `_ngram_overlap_score`, `_merge_results`,
//! `_hive_fact_to_dict`, and `_restore_hive_metadata` from Python
//! `cognitive_adapter.py`.

use std::collections::HashMap;

use serde_json::Value;

use super::constants::{BIGRAM_WEIGHT, QUERY_STOP_WORDS, UNIGRAM_WEIGHT};
use super::types::HiveFact;
use crate::agentic_loop::types::MemoryFact;

/// Strip common punctuation from a word boundary.
fn strip_punct(w: &str) -> &str {
    w.trim_matches(|c: char| "?.,!;:'\"()[]".contains(c))
}

/// Remove stop words from `query`, preserving meaningful terms.
///
/// Returns the filtered string, or the original lowered query if all
/// words are stop words.
pub fn filter_stop_words(query: &str) -> String {
    let words: Vec<&str> = query
        .split_whitespace()
        .map(strip_punct)
        .filter(|w| !w.is_empty())
        .collect();

    let filtered: Vec<&&str> = words
        .iter()
        .filter(|w| {
            let lower = w.to_lowercase();
            lower.len() > 1 && !QUERY_STOP_WORDS.contains(lower.as_str())
        })
        .collect();

    if filtered.is_empty() {
        query.to_lowercase()
    } else {
        filtered
            .iter()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Score `text` by unigram + bigram overlap with `query`.
///
/// Returns a float in `[0.0, 1.0]` where higher = more overlap.
/// Uses the same stop-word–filtered unigram matching and raw-bigram
/// matching as the Python implementation.
pub fn ngram_overlap_score(query: &str, text: &str) -> f64 {
    let q_words: Vec<String> = query
        .split_whitespace()
        .map(|w| strip_punct(w).to_lowercase())
        .collect();

    let t_words: Vec<String> = text.split_whitespace().map(|w| w.to_lowercase()).collect();

    // Unigram overlap (stop-word filtered)
    let q_terms: Vec<&str> = q_words
        .iter()
        .filter(|w| !w.is_empty() && w.len() > 1 && !QUERY_STOP_WORDS.contains(w.as_str()))
        .map(|s| s.as_str())
        .collect();

    let unigram = if q_terms.is_empty() {
        0.0
    } else {
        let hits: usize = q_terms
            .iter()
            .filter(|t| {
                t_words.iter().any(|w| w == **t)
                    || t_words
                        .iter()
                        .any(|w| w.len() > 2 && (w.starts_with(**t) || t.starts_with(w.as_str())))
            })
            .count();
        hits as f64 / q_terms.len() as f64
    };

    // Bigram overlap
    let q_bigrams: Vec<(&str, &str)> = q_words
        .windows(2)
        .map(|w| (w[0].as_str(), w[1].as_str()))
        .collect();

    let t_bigrams: std::collections::HashSet<(&str, &str)> = t_words
        .windows(2)
        .map(|w| (w[0].as_str(), w[1].as_str()))
        .collect();

    let bigram = if q_bigrams.is_empty() {
        0.0
    } else {
        let hits = q_bigrams.iter().filter(|bg| t_bigrams.contains(bg)).count();
        hits as f64 / q_bigrams.len() as f64
    };

    unigram * UNIGRAM_WEIGHT + bigram * BIGRAM_WEIGHT
}

/// Re-rank results by n-gram overlap with the original query.
pub fn rerank_by_ngram(query: &str, results: &mut Vec<MemoryFact>, limit: usize) {
    if results.is_empty() {
        return;
    }
    let mut scored: Vec<(f64, MemoryFact)> = results
        .drain(..)
        .map(|r| {
            let text = format!("{} {}", r.context, r.outcome);
            let score = ngram_overlap_score(query, &text);
            (score, r)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    results.extend(scored.into_iter().map(|(_, r)| r));
}

/// Merge local and hive results, deduplicating by content.
///
/// Local facts are preferred (higher trust).
pub fn merge_results(local: &[MemoryFact], hive: &[MemoryFact], limit: usize) -> Vec<MemoryFact> {
    let mut seen = std::collections::HashSet::new();
    let mut merged = Vec::with_capacity(limit);

    for r in local.iter().chain(hive.iter()) {
        if !r.outcome.is_empty() && seen.insert(r.outcome.clone()) {
            merged.push(r.clone());
            if merged.len() >= limit {
                break;
            }
        }
    }
    merged
}

/// Convert a [`HiveFact`] to the standard [`MemoryFact`] format.
///
/// Restores temporal metadata from hive tags and adds a `source` key.
pub fn hive_fact_to_memory(hf: HiveFact) -> MemoryFact {
    let mut meta = hf.metadata;
    restore_hive_metadata(&hf.tags, &mut meta);
    meta.insert(
        "source".into(),
        Value::String(format!("hive:{}", hf.source_agent)),
    );
    MemoryFact {
        id: hf.fact_id,
        context: hf.concept,
        outcome: hf.content,
        confidence: hf.confidence,
        metadata: meta,
    }
}

/// Rehydrate temporal metadata from hive tags (`date:…`, `time:…`).
fn restore_hive_metadata(tags: &[String], meta: &mut HashMap<String, Value>) {
    for tag in tags {
        if let Some(date) = tag.strip_prefix("date:") {
            if !meta.contains_key("source_date") {
                meta.insert("source_date".into(), Value::String(date.to_string()));
                if !meta.contains_key("temporal_index") {
                    let digits: String = date.chars().filter(|c| c.is_ascii_digit()).collect();
                    if let Ok(idx) = digits.parse::<i64>() {
                        meta.insert(
                            "temporal_index".into(),
                            serde_json::to_value(idx).unwrap_or(Value::Null),
                        );
                    }
                }
            }
        } else if let Some(time) = tag.strip_prefix("time:")
            && !meta.contains_key("temporal_order")
        {
            meta.insert("temporal_order".into(), Value::String(time.to_string()));
            if !meta.contains_key("temporal_index") {
                let digits: String = time.chars().filter(|c| c.is_ascii_digit()).collect();
                if let Ok(idx) = digits.parse::<i64>() {
                    meta.insert(
                        "temporal_index".into(),
                        serde_json::to_value(idx).unwrap_or(Value::Null),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn filter_stop_words_basic() {
        assert_eq!(filter_stop_words("what is the capital"), "capital");
    }

    #[test]
    fn filter_stop_words_preserves_meaningful() {
        let result = filter_stop_words("cells basic unit life");
        assert!(result.contains("cells"));
        assert!(result.contains("basic"));
    }

    #[test]
    fn filter_stop_words_all_stop() {
        let result = filter_stop_words("is the a");
        // Falls back to lowered original
        assert_eq!(result, "is the a");
    }

    #[test]
    fn ngram_score_perfect_overlap() {
        let score = ngram_overlap_score("cells biology", "cells biology");
        assert!(score > 0.5, "score={score}");
    }

    #[test]
    fn ngram_score_no_overlap() {
        let score = ngram_overlap_score("quantum physics", "baking recipes flour");
        assert!(score < 0.01, "score={score}");
    }

    #[test]
    fn ngram_score_partial_match() {
        let score = ngram_overlap_score("cell biology", "cells are the basic unit");
        assert!(score > 0.0, "score={score}");
    }

    #[test]
    fn merge_deduplicates() {
        let f1 = MemoryFact {
            id: "1".into(),
            context: "bio".into(),
            outcome: "cells".into(),
            confidence: 0.9,
            metadata: HashMap::new(),
        };
        let f2 = MemoryFact {
            id: "2".into(),
            context: "bio".into(),
            outcome: "cells".into(), // duplicate
            confidence: 0.8,
            metadata: HashMap::new(),
        };
        let f3 = MemoryFact {
            id: "3".into(),
            context: "chem".into(),
            outcome: "atoms".into(),
            confidence: 0.7,
            metadata: HashMap::new(),
        };
        let merged = merge_results(&[f1], &[f2, f3], 10);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].outcome, "cells");
        assert_eq!(merged[1].outcome, "atoms");
    }

    #[test]
    fn merge_respects_limit() {
        let facts: Vec<MemoryFact> = (0..10)
            .map(|i| MemoryFact {
                id: format!("{i}"),
                context: "ctx".into(),
                outcome: format!("fact-{i}"),
                confidence: 0.9,
                metadata: HashMap::new(),
            })
            .collect();
        let merged = merge_results(&facts, &[], 3);
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn rerank_orders_by_relevance() {
        let mut results = vec![
            MemoryFact {
                id: "1".into(),
                context: "cooking".into(),
                outcome: "use flour for baking".into(),
                confidence: 0.9,
                metadata: HashMap::new(),
            },
            MemoryFact {
                id: "2".into(),
                context: "biology".into(),
                outcome: "cells are the basic unit of life".into(),
                confidence: 0.9,
                metadata: HashMap::new(),
            },
        ];
        rerank_by_ngram("cell biology", &mut results, 10);
        assert_eq!(results[0].id, "2");
    }
}
