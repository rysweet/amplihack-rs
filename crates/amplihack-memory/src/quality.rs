//! Quality control helpers for the memory coordinator.
//!
//! Extracted from coordinator.rs to keep modules under 400 lines.
//! Contains: trivial content filter, duplicate detection, importance
//! scoring, and relevance ranking.

use crate::models::{MemoryEntry, MemoryQuery, MemoryType};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

/// Common trivial phrases that should be rejected.
const TRIVIAL_PHRASES: &[&str] = &[
    "hello",
    "hi",
    "hey",
    "thanks",
    "thank you",
    "ok",
    "okay",
    "yes",
    "no",
    "sure",
    "got it",
    "understood",
    "sounds good",
];

/// Check if content is trivial (too short or common greeting).
pub fn is_trivial(content: &str, min_length: usize) -> bool {
    let trimmed = content.trim();
    if trimmed.len() < min_length {
        return true;
    }
    let lower = trimmed.to_lowercase();
    TRIVIAL_PHRASES.iter().any(|&p| lower == p)
}

/// Check if a memory entry matches a query's filters.
pub fn matches_query(entry: &MemoryEntry, query: &MemoryQuery) -> bool {
    if let Some(ref sid) = query.session_id
        && entry.session_id != *sid
    {
        return false;
    }
    if let Some(ref aid) = query.agent_id
        && entry.agent_id != *aid
    {
        return false;
    }
    if !query.memory_types.is_empty() && !query.memory_types.contains(&entry.memory_type) {
        return false;
    }
    if !query.tags.is_empty() && !query.tags.iter().any(|t| entry.tags.contains(t)) {
        return false;
    }
    if let Some(range) = query.time_range_secs {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        if now - entry.created_at > range {
            return false;
        }
    }
    true
}

/// Score relevance of an entry against query words.
///
/// Combines word overlap (60%), recency boost (20%), and importance (20%).
pub fn relevance_score(entry: &MemoryEntry, query_words: &HashSet<&str>) -> f64 {
    let content_words: HashSet<&str> = entry.content.split_whitespace().collect();
    let overlap = query_words.intersection(&content_words).count() as f64;
    let word_score = if query_words.is_empty() {
        0.0
    } else {
        overlap / query_words.len() as f64
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let age_hours = (now - entry.created_at) / 3600.0;
    let recency_boost = 1.0 / (1.0 + age_hours * 0.1);

    word_score * 0.6 + recency_boost * 0.2 + entry.importance * 0.2
}

/// Score the importance of content based on type and heuristics.
pub fn score_importance(content: &str, memory_type: MemoryType) -> f64 {
    let mut score: f64 = 0.5;

    score += match memory_type {
        MemoryType::Procedural => 0.1,
        MemoryType::Strategic => 0.15,
        MemoryType::Working => -0.1,
        _ => 0.0,
    };

    let len = content.len();
    if len > 200 {
        score += 0.1;
    }
    if len > 500 {
        score += 0.05;
    }

    if content.contains("fn ") || content.contains("def ") || content.contains("class ") {
        score += 0.1;
    }

    score.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trivial_detection() {
        assert!(is_trivial("hi", 10));
        assert!(is_trivial("thanks", 10));
        assert!(is_trivial("short", 10));
        assert!(!is_trivial(
            "This is substantive content that passes the filter",
            10
        ));
    }

    #[test]
    fn importance_scoring() {
        assert!(
            score_importance("fn main() { println!(\"hello\"); }", MemoryType::Procedural) > 0.6
        );
        assert!(score_importance("ok", MemoryType::Working) < 0.5);
    }

    #[test]
    fn relevance_with_empty_query() {
        let entry = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "test");
        let empty: HashSet<&str> = HashSet::new();
        let score = relevance_score(&entry, &empty);
        // Should still return non-zero (recency + importance)
        assert!(score > 0.0);
    }

    #[test]
    fn query_filter_by_session() {
        let entry = MemoryEntry::new("s1", "a1", MemoryType::Semantic, "data");
        let q = MemoryQuery::new("data").with_session("s1");
        assert!(matches_query(&entry, &q));
        let q2 = MemoryQuery::new("data").with_session("other");
        assert!(!matches_query(&entry, &q2));
    }

    #[test]
    fn query_filter_by_type() {
        let entry = MemoryEntry::new("s1", "a1", MemoryType::Working, "data");
        let q = MemoryQuery::new("data").with_types(vec![MemoryType::Working]);
        assert!(matches_query(&entry, &q));
        let q2 = MemoryQuery::new("data").with_types(vec![MemoryType::Semantic]);
        assert!(!matches_query(&entry, &q2));
    }
}
