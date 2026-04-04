//! Core retrieval strategies: simple, tiered, entity, and concept.
//!
//! Ported from Python `retrieval_strategies.py` `RetrievalStrategiesMixin`.

use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

use super::constants::*;
use super::scoring::fact_mentions_entity;
use super::types::{Fact, MemorySearch};

/// Compiled pattern for structured entity IDs like `INC-2024-001`.
pub(crate) static ENTITY_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Z]{2,5}-\d{4}-\d{2,5})\b").expect("valid regex"));

/// Proper-noun extraction: multi-word names like "Sarah Chen", "Al-Hassan".
static PROPER_NOUN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)\b(
        [A-Z][a-z]*(?:['\x{2019}\-][A-Z]?[a-z]+)+(?:\s+(?:[A-Z][a-z]+(?:['\x{2019}\-][A-Z]?[a-z]+)?))*
        |
        [A-Z][a-z]+(?:\s+(?:[A-Z][a-z]+(?:['\x{2019}\-][A-Z]?[a-z]+)?))+
        )\b",
    )
    .expect("valid regex")
});

/// Possessive suffix pattern: `Name's`.
static POSSESSIVE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)\b(
        [A-Z][a-z]*(?:['\x{2019}\-][A-Z]?[a-z]+)+(?:\s+(?:[A-Z][a-z]+(?:['\x{2019}\-][A-Z]?[a-z]+)?))*
        |
        [A-Z][a-z]+(?:\s+(?:[A-Z][a-z]+(?:['\x{2019}\-][A-Z]?[a-z]+)?))*
        )'s\b",
    )
    .expect("valid regex")
});

// Stop words for concept retrieval (same set as Python).
static STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can",
        "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "about",
        "between", "through", "during", "before", "after", "above", "below", "and", "but", "or",
        "not", "no", "if", "then", "than", "that", "this", "these", "those", "it", "its", "he",
        "she", "they", "we", "you", "i", "my", "your", "his", "her", "their", "our", "what",
        "which", "who", "whom", "how", "when", "where", "why", "all", "each", "every", "both",
        "few", "more", "most", "other", "some", "such", "any", "many", "much", "own", "same", "so",
        "too", "very", "just", "also",
    ]
    .into_iter()
    .collect()
});

/// Extract structured entity IDs from text.
pub fn extract_entity_ids(text: &str) -> Vec<String> {
    ENTITY_ID_RE
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Single-pass retrieval with progressive summarisation for large KBs.
///
/// - Small KBs (≤ `VERBATIM_RETRIEVAL_THRESHOLD`): all facts verbatim.
/// - Large KBs: tiered summarisation.
///
/// Returns `(facts, was_exhaustive)`.
pub fn simple_retrieval(
    memory: &dyn MemorySearch,
    question: &str,
    force_verbatim: bool,
    pre_snapshot: Option<&[Fact]>,
) -> (Vec<Fact>, bool) {
    if let Some(snapshot) = pre_snapshot {
        let exhaustive = force_verbatim || snapshot.len() <= VERBATIM_RETRIEVAL_THRESHOLD;
        if exhaustive {
            return (snapshot.to_vec(), true);
        }
        return (tiered_retrieval(question, snapshot), false);
    }

    let all_facts = memory.get_all_facts(MAX_RETRIEVAL_LIMIT, question);
    let kb_size = all_facts.len();

    let total_est = estimate_total_fact_count(memory, None);
    let distributed_partial =
        memory.supports_local_search() && total_est.is_some() && kb_size > total_est.unwrap_or(0);

    let exhaustive = force_verbatim
        || (!distributed_partial
            && ((total_est.is_some()
                && total_est.unwrap() <= VERBATIM_RETRIEVAL_THRESHOLD
                && kb_size >= total_est.unwrap())
                || (total_est.is_none()
                    && !memory.supports_local_search()
                    && kb_size <= VERBATIM_RETRIEVAL_THRESHOLD)));

    if exhaustive {
        return (all_facts, true);
    }
    (tiered_retrieval(question, &all_facts), false)
}

/// Estimate the total locally stored fact count.
pub fn estimate_total_fact_count(
    memory: &dyn MemorySearch,
    pre_snapshot: Option<&[Fact]>,
) -> Option<usize> {
    if let Some(snap) = pre_snapshot {
        return Some(snap.len());
    }
    memory.get_statistics()?.estimated_total()
}

/// Tiered retrieval with progressive summarisation.
///
/// - Tier 1 (most recent `TIER1_VERBATIM_SIZE`): verbatim.
/// - Tier 2 (next `TIER2_ENTITY_SIZE - TIER1_VERBATIM_SIZE`): entity-level summaries.
/// - Tier 3 (older): topic-level summaries.
pub fn tiered_retrieval(question: &str, all_facts: &[Fact]) -> Vec<Fact> {
    let mut sorted: Vec<Fact> = all_facts.to_vec();
    sorted.sort_by_key(|a| a.temporal_sort_key());

    let (exact_id_facts, remaining) = preserve_exact_id_facts(question, &sorted);
    let mut result: Vec<Fact> = exact_id_facts.clone();

    // Tier 1: verbatim
    let tier1_start = remaining.len().saturating_sub(TIER1_VERBATIM_SIZE);
    let tier1_facts = &remaining[tier1_start..];
    result.extend_from_slice(tier1_facts);

    // Tier 2: entity-level summaries
    let tier2_start = remaining.len().saturating_sub(TIER2_ENTITY_SIZE);
    let tier2_end = remaining.len().saturating_sub(TIER1_VERBATIM_SIZE);
    if tier2_end > tier2_start {
        let summaries = summarize_old_facts(&remaining[tier2_start..tier2_end], "entity");
        result.extend(summaries);
    }

    // Tier 3: topic-level summaries
    if tier2_start > 0 {
        let summaries = summarize_old_facts(&remaining[..tier2_start], "topic");
        result.extend(summaries);
    }

    result
}

/// Keep question-matching structured-ID facts verbatim before tiering.
fn preserve_exact_id_facts(question: &str, sorted_facts: &[Fact]) -> (Vec<Fact>, Vec<Fact>) {
    let entity_ids: HashSet<String> = extract_entity_ids(question)
        .into_iter()
        .map(|id| id.to_lowercase())
        .collect();
    if entity_ids.is_empty() {
        return (Vec::new(), sorted_facts.to_vec());
    }

    let q_lower = question.to_lowercase();
    let is_incident = ["incident", "cve", "vulnerability", "security"]
        .iter()
        .any(|kw| q_lower.contains(kw));
    let exact_limit = if is_incident {
        INCIDENT_QUERY_SEARCH_LIMIT
    } else {
        ENTITY_SEARCH_LIMIT
    };

    let mut exact_facts = Vec::new();
    let mut remaining = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for fact in sorted_facts {
        if fact_mentions_entity(fact, &entity_ids) {
            let key = fact.dedup_key();
            if seen.insert(key) {
                exact_facts.push(fact.clone());
            }
        } else {
            remaining.push(fact.clone());
        }
    }

    if exact_facts.len() > exact_limit {
        let start = exact_facts.len() - exact_limit;
        exact_facts = exact_facts[start..].to_vec();
    }

    (exact_facts, remaining)
}

/// Deterministic summarisation: group facts by entity or topic, no LLM calls.
pub fn summarize_old_facts(facts: &[Fact], level: &str) -> Vec<Fact> {
    if facts.is_empty() {
        return Vec::new();
    }

    let mut groups: std::collections::HashMap<String, Vec<&Fact>> =
        std::collections::HashMap::new();
    for f in facts {
        let key = if level == "entity" {
            if f.context.is_empty() {
                "General".to_string()
            } else {
                f.context.clone()
            }
        } else {
            let ctx = if f.context.is_empty() {
                "General"
            } else {
                &f.context
            };
            if let Some(pos) = ctx.find(':') {
                ctx[..pos].trim().to_string()
            } else if let Some(pos) = ctx.find('.') {
                ctx[..pos].trim().to_string()
            } else {
                ctx.to_string()
            }
        };
        groups.entry(key).or_default().push(f);
    }

    let mut summaries = Vec::new();
    for (group_key, group_facts) in &groups {
        if group_facts.len() <= 2 {
            summaries.extend(group_facts.iter().map(|f| (*f).clone()));
            continue;
        }

        let texts: Vec<String> = group_facts
            .iter()
            .take(SUMMARY_GROUP_MAX_FACTS)
            .filter_map(|f| {
                let text = &f.outcome;
                if text.is_empty() {
                    None
                } else {
                    let truncated: String = text.chars().take(SUMMARY_FACT_TEXT_MAX_LEN).collect();
                    Some(truncated)
                }
            })
            .collect();

        if texts.is_empty() {
            continue;
        }

        let mut combined = texts.join("; ");
        if combined.len() > SUMMARY_COMBINED_TARGET_LEN {
            if let Some(pos) = combined[..SUMMARY_COMBINED_TARGET_LEN].rfind('.') {
                if pos > SUMMARY_TRUNCATE_MIN_OFFSET {
                    combined.truncate(pos + 1);
                } else {
                    combined.truncate(SUMMARY_COMBINED_TARGET_LEN);
                    combined.push_str("...");
                }
            } else {
                combined.truncate(SUMMARY_COMBINED_TARGET_LEN);
                combined.push_str("...");
            }
        }

        summaries.push(Fact::summary(
            group_key,
            group_facts.len(),
            &combined,
            level,
        ));
    }
    summaries
}

/// Entity-centric retrieval for questions about specific people/projects.
pub fn entity_retrieval(memory: &dyn MemorySearch, question: &str, local_only: bool) -> Vec<Fact> {
    if !memory.supports_hierarchical() {
        return Vec::new();
    }

    let mut candidates: Vec<String> = PROPER_NOUN_RE
        .find_iter(question)
        .map(|m| m.as_str().to_string())
        .collect();

    // Fallback: single capitalised words > 2 chars
    if candidates.is_empty() {
        for word in question.split_whitespace() {
            let cleaned = word.trim_matches(|c: char| c.is_ascii_punctuation());
            if cleaned.len() > 2 && cleaned.starts_with(|c: char| c.is_uppercase()) {
                candidates.push(cleaned.to_string());
            }
        }
    }

    // Possessives: "Fatima's" → "Fatima"
    for m in POSSESSIVE_RE.find_iter(question) {
        candidates.push(m.as_str().trim_end_matches("'s").to_string());
    }

    if candidates.is_empty() {
        return Vec::new();
    }

    let mut all_facts = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for candidate in &candidates {
        let entity_facts = if local_only {
            memory.retrieve_by_entity_local(candidate, ENTITY_FACT_LIMIT)
        } else {
            memory.retrieve_by_entity(candidate, ENTITY_FACT_LIMIT)
        };
        for fact in entity_facts {
            if seen.insert(fact.dedup_key()) {
                all_facts.push(fact);
            }
        }
    }
    all_facts
}

/// Concept-based retrieval fallback: extract key noun phrases, search memory.
pub fn concept_retrieval(memory: &dyn MemorySearch, question: &str, local_only: bool) -> Vec<Fact> {
    if !memory.supports_hierarchical() {
        return Vec::new();
    }

    let words: Vec<String> = question
        .split_whitespace()
        .filter_map(|w| {
            let cleaned = w
                .trim_matches(|c: char| c.is_ascii_punctuation())
                .to_lowercase();
            if cleaned.len() > 2 && !STOP_WORDS.contains(cleaned.as_str()) {
                Some(cleaned)
            } else {
                None
            }
        })
        .collect();

    if words.is_empty() {
        return Vec::new();
    }

    // Bigrams first, then individual words
    let mut phrases: Vec<String> = words
        .windows(2)
        .map(|w| format!("{} {}", w[0], w[1]))
        .collect();
    phrases.extend(words);

    let mut all_facts = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for phrase in phrases.iter().take(CONCEPT_PHRASE_LIMIT) {
        let kw = vec![phrase.clone()];
        let results = if local_only {
            memory.search_by_concept_local(&kw, CONCEPT_SEARCH_LIMIT)
        } else {
            memory.search_by_concept(&kw, CONCEPT_SEARCH_LIMIT)
        };
        for fact in results {
            let key = fact.dedup_key();
            if !key.is_empty() && seen.insert(key) {
                all_facts.push(fact);
            }
        }
    }
    all_facts
}
