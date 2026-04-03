//! Text similarity computation for knowledge graph edges.
//!
//! Port of Python `similarity.py` — deterministic text similarity using
//! Jaccard coefficients on tokenized words (no ML embeddings needed).

use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::memory_retrieval::SearchResult;

// ── Stop words ───────────────────────────────────────────────────────────

static STOP_WORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    "a an the is are was were be been being have has had do does did will would could \
     should may might shall can to of in for on with at by from as into about like \
     through after over between out against during without before under around among \
     and but or nor not so yet both either neither each every all any few more most \
     other some such no only own same than too very just because if when where how \
     what which who whom this that these those it its i me my we our you your he him \
     his she her they them their"
        .split_whitespace()
        .collect()
});

static QUERY_CUE_TOKENS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    "change changed current currently different evolution first history initial \
     initially latest list many name now number original originally over previous \
     project revised show status time timeline updated"
        .split_whitespace()
        .collect()
});

static APT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\bapt(?:-| )?\d+\b").unwrap());

// ── Tokenization ─────────────────────────────────────────────────────────

const PUNCT: &[char] = &['.', ',', ';', ':', '!', '?', '(', ')', '[', ']', '{', '}', '"', '\''];

fn tokenize(text: &str) -> HashSet<String> {
    if text.is_empty() {
        return HashSet::new();
    }
    text.to_lowercase()
        .split_whitespace()
        .map(|w| w.trim_matches(PUNCT).to_string())
        .filter(|w| w.len() > 2 && !STOP_WORDS.contains(w.as_str()))
        .collect()
}

fn ordered_tokens(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.to_lowercase()
        .split_whitespace()
        .filter_map(|w| {
            let cleaned = w.trim_matches(PUNCT).to_string();
            if cleaned.len() > 2 && !STOP_WORDS.contains(cleaned.as_str()) {
                Some(cleaned)
            } else {
                None
            }
        })
        .collect()
}

fn anchor_tokens(query: &str) -> HashSet<String> {
    let tokens = tokenize(query);
    let anchors: HashSet<String> = tokens
        .iter()
        .filter(|t| !QUERY_CUE_TOKENS.contains(t.as_str()))
        .cloned()
        .collect();
    if anchors.is_empty() { tokens } else { anchors }
}

fn query_phrases(query: &str) -> HashSet<String> {
    let ordered = ordered_tokens(query);
    let mut phrases = HashSet::new();
    for size in [3, 2] {
        if ordered.len() < size {
            continue;
        }
        for idx in 0..=ordered.len() - size {
            let window = &ordered[idx..idx + size];
            let non_cue: usize = window
                .iter()
                .filter(|t| !QUERY_CUE_TOKENS.contains(t.as_str()))
                .count();
            if non_cue == 0 || (size == 3 && non_cue < 2) {
                continue;
            }
            let phrase = window.join(" ");
            if phrase.len() >= 8 {
                phrases.insert(phrase);
            }
        }
    }
    phrases
}

fn entity_anchor_tokens(query: &str) -> HashSet<String> {
    let mut anchors = HashSet::new();
    for raw in query.split_whitespace() {
        let cleaned = raw.trim_matches(PUNCT);
        let lowered = cleaned.to_lowercase();
        if cleaned.len() > 2
            && cleaned.chars().any(|c| c.is_uppercase())
            && !STOP_WORDS.contains(lowered.as_str())
            && !QUERY_CUE_TOKENS.contains(lowered.as_str())
        {
            anchors.insert(lowered);
        }
    }
    anchors
}

// ── Public token extractors ──────────────────────────────────────────────

/// Expose discriminative query anchors.
pub fn extract_query_anchor_tokens(query: &str) -> HashSet<String> { anchor_tokens(query) }

/// Expose discriminative ordered phrases.
pub fn extract_query_phrases(query: &str) -> HashSet<String> { query_phrases(query) }

/// Expose likely entity-identifying query tokens.
pub fn extract_entity_anchor_tokens(query: &str) -> HashSet<String> { entity_anchor_tokens(query) }

/// Expose tokenization for downstream use.
pub fn tokenize_similarity_text(text: &str) -> HashSet<String> { tokenize(text) }

// ── Similarity functions ─────────────────────────────────────────────────

/// Compute Jaccard similarity on tokenized words minus stop words.
pub fn compute_word_similarity(text_a: &str, text_b: &str) -> f64 {
    let (a, b) = (tokenize(text_a), tokenize(text_b));
    if a.is_empty() || b.is_empty() { return 0.0; }
    let union = a.union(&b).count();
    if union == 0 { 0.0 } else { a.intersection(&b).count() as f64 / union as f64 }
}

/// Compute Jaccard similarity between two tag lists.
pub fn compute_tag_similarity(tags_a: &[String], tags_b: &[String]) -> f64 {
    if tags_a.is_empty() || tags_b.is_empty() { return 0.0; }
    let to_set = |tags: &[String]| -> HashSet<String> {
        tags.iter().map(|t| t.to_lowercase().trim().to_string()).filter(|t| !t.is_empty()).collect()
    };
    let (a, b) = (to_set(tags_a), to_set(tags_b));
    if a.is_empty() || b.is_empty() { return 0.0; }
    let union = a.union(&b).count();
    if union == 0 { 0.0 } else { a.intersection(&b).count() as f64 / union as f64 }
}

/// Weighted composite similarity: 0.5×word + 0.2×tag + 0.3×concept.
pub fn compute_similarity(node_a: &NodeSimilarityInput, node_b: &NodeSimilarityInput) -> f64 {
    let word_sim = compute_word_similarity(&node_a.content, &node_b.content);
    let tag_sim = compute_tag_similarity(&node_a.tags, &node_b.tags);
    let concept_sim = compute_word_similarity(&node_a.concept, &node_b.concept);
    0.5 * word_sim + 0.2 * tag_sim + 0.3 * concept_sim
}

/// Input for [`compute_similarity`] — mirrors the Python dict interface.
#[derive(Debug, Clone, Default)]
pub struct NodeSimilarityInput {
    pub content: String,
    pub tags: Vec<String>,
    pub concept: String,
}

// ── Reranking ────────────────────────────────────────────────────────────

static TEMPORAL_CUES: Lazy<Vec<&'static str>> = Lazy::new(|| {
    vec!["change", "changed", "original", "before", "after", "previous", "current",
         "first", "initially", "updated", "revised", "intermediate", "over time",
         "history", "evolution", "timeline", "when"]
});

/// Rerank retrieved facts by keyword relevance to a query.
pub fn rerank_facts_by_query(
    facts: &[SearchResult],
    query: &str,
    top_k: usize,
) -> Vec<SearchResult> {
    if facts.is_empty() || query.is_empty() {
        return facts.to_vec();
    }
    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        return facts.to_vec();
    }
    let anchors = anchor_tokens(query);
    let phrases = query_phrases(query);
    let query_lower = query.to_lowercase();

    let has_temporal = TEMPORAL_CUES.iter().any(|c| query_lower.contains(c));
    let has_apt_attribution = query_lower.contains("apt")
        && ["attributed", "group", "threat actor"]
            .iter()
            .any(|c| query_lower.contains(c));

    let mut scored: Vec<(f64, usize, SearchResult)> = facts
        .iter()
        .enumerate()
        .map(|(idx, fact)| {
            let fact_text = format!("{} {}", fact.context, fact.outcome);
            let fact_lower = fact_text.to_lowercase();
            let fact_tokens = tokenize(&fact_text);

            if fact_tokens.is_empty() {
                return (0.0, idx, fact.clone());
            }

            let overlap_base =
                query_tokens.intersection(&fact_tokens).count() as f64
                    / query_tokens.len() as f64;
            let anchor_overlap = if anchors.is_empty() {
                0.0
            } else {
                anchors.intersection(&fact_tokens).count() as f64 / anchors.len() as f64
            };
            let phrase_hits = phrases.iter().filter(|p| fact_lower.contains(p.as_str())).count();
            let phrase_bonus = if phrases.is_empty() {
                0.0
            } else {
                0.25 * (phrase_hits as f64 / phrases.len() as f64)
            };
            let mut score = overlap_base + 0.6 * anchor_overlap + phrase_bonus;

            if has_temporal {
                let has_temporal_meta = fact
                    .metadata
                    .get("temporal_index")
                    .and_then(|v| v.as_i64())
                    .is_some_and(|i| i > 0)
                    || fact.metadata.contains_key("source_date")
                    || fact.metadata.contains_key("temporal_order");
                if has_temporal_meta {
                    score += 0.15;
                }
            }

            if has_apt_attribution {
                if APT_RE.is_match(&fact_text) {
                    score += 0.35;
                }
                if fact_lower.contains("matched apt") || fact_lower.contains("ttp") {
                    score += 0.1;
                }
            }

            let is_summary = fact
                .metadata
                .get("is_summary")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if is_summary && phrase_hits == 0 && anchor_overlap < 1.0 {
                score -= 0.05;
            }

            (score, idx, fact.clone())
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.1.cmp(&b.1)));

    let reranked: Vec<SearchResult> = scored.into_iter().map(|(_, _, f)| f).collect();
    if top_k > 0 { reranked.into_iter().take(top_k).collect() } else { reranked }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn tokenize_basic() {
        let tokens = tokenize("The quick brown fox jumps over the lazy dog");
        assert!(tokens.contains("quick"));
        assert!(tokens.contains("brown"));
        assert!(!tokens.contains("the"));
        assert!(!tokens.contains("over"));
    }

    #[test]
    fn tokenize_empty() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("a to is").is_empty());
    }

    #[test]
    fn tokenize_strips_punctuation() {
        let tokens = tokenize("hello, world! (test)");
        assert!(tokens.contains("hello"));
        assert!(tokens.contains("world"));
        assert!(tokens.contains("test"));
    }

    #[test]
    fn word_similarity_identical() {
        let sim = compute_word_similarity("photosynthesis converts light", "photosynthesis converts light");
        assert!((sim - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn word_similarity_disjoint() {
        let sim = compute_word_similarity("photosynthesis plants", "quantum mechanics");
        assert!(sim < f64::EPSILON);
    }

    #[test]
    fn word_similarity_cases() {
        let sim = compute_word_similarity("photosynthesis plants energy", "photosynthesis light energy");
        assert!(sim > 0.0 && sim < 1.0);
        assert!(compute_word_similarity("", "test").abs() < f64::EPSILON);
        assert!(compute_word_similarity("test", "").abs() < f64::EPSILON);
    }

    #[test]
    fn tag_similarity_cases() {
        let (a, b) = (vec!["biology".into(), "science".into()], vec!["biology".into(), "science".into()]);
        assert!((compute_tag_similarity(&a, &b) - 1.0).abs() < f64::EPSILON);
        assert!(compute_tag_similarity(&[], &["x".into()]).abs() < f64::EPSILON);
        assert!((compute_tag_similarity(&["Biology".into()], &["biology".into()]) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn composite_similarity() {
        let a = NodeSimilarityInput { content: "cells unit life".into(), tags: vec!["bio".into()], concept: "cell bio".into() };
        let b = NodeSimilarityInput { content: "cells mitosis".into(), tags: vec!["bio".into()], concept: "cell div".into() };
        let sim = compute_similarity(&a, &b);
        assert!(sim > 0.0 && sim <= 1.0);
    }

    #[test]
    fn token_extractors() {
        let anchors = extract_query_anchor_tokens("What is the current status of Mars project");
        assert!(!anchors.contains("current") && anchors.contains("mars"));
        let ent = extract_entity_anchor_tokens("What happened to Mars Rover in 2024");
        assert!(ent.contains("mars") && ent.contains("rover"));
        assert!(!extract_query_phrases("the quick brown fox jumps over lazy dog").is_empty());
        assert!(tokenize_similarity_text("Hello world testing").contains("hello"));
    }

    fn sr(id: &str, ctx: &str, outcome: &str) -> SearchResult {
        SearchResult {
            experience_id: id.into(), context: ctx.into(), outcome: outcome.into(),
            confidence: 0.8, timestamp: String::new(), tags: vec![], metadata: HashMap::new(),
        }
    }

    #[test]
    fn rerank_empty() {
        assert!(rerank_facts_by_query(&[], "test", 0).is_empty());
    }

    #[test]
    fn rerank_orders_by_relevance() {
        let facts = vec![sr("1", "Cooking", "Pasta recipe with garlic"),
                         sr("2", "Biology", "Photosynthesis converts light energy")];
        let reranked = rerank_facts_by_query(&facts, "photosynthesis light", 0);
        assert_eq!(reranked[0].experience_id, "2");
    }

    #[test]
    fn rerank_top_k() {
        let facts = vec![sr("1", "A", "alpha beta"), sr("2", "B", "gamma delta")];
        assert_eq!(rerank_facts_by_query(&facts, "alpha", 1).len(), 1);
    }

    #[test]
    fn tokenize_similarity_text_public() {
        let tokens = tokenize_similarity_text("Hello world testing");
        assert!(tokens.contains("hello"));
        assert!(tokens.contains("world"));
    }
}
