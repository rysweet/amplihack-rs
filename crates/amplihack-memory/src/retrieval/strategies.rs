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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::MemoryStatistics;

    // -----------------------------------------------------------------------
    // Mock MemorySearch
    // -----------------------------------------------------------------------

    struct MockMemory {
        facts: Vec<Fact>,
        hierarchical: bool,
        local_search: bool,
        stats: Option<MemoryStatistics>,
    }

    impl MockMemory {
        fn new(facts: Vec<Fact>) -> Self {
            Self {
                facts,
                hierarchical: false,
                local_search: false,
                stats: None,
            }
        }

        fn with_hierarchical(mut self) -> Self {
            self.hierarchical = true;
            self
        }

        fn with_stats(mut self, stats: MemoryStatistics) -> Self {
            self.stats = Some(stats);
            self
        }
    }

    impl MemorySearch for MockMemory {
        fn get_all_facts(&self, limit: usize, _query: &str) -> Vec<Fact> {
            self.facts.iter().take(limit).cloned().collect()
        }

        fn search(&self, query: &str, limit: usize) -> Vec<Fact> {
            let q = query.to_lowercase();
            self.facts
                .iter()
                .filter(|f| {
                    f.context.to_lowercase().contains(&q)
                        || f.outcome.to_lowercase().contains(&q)
                })
                .take(limit)
                .cloned()
                .collect()
        }

        fn supports_hierarchical(&self) -> bool {
            self.hierarchical
        }

        fn supports_local_search(&self) -> bool {
            self.local_search
        }

        fn get_statistics(&self) -> Option<MemoryStatistics> {
            self.stats.clone()
        }
    }

    fn make_fact(ctx: &str, outcome: &str) -> Fact {
        Fact::new(ctx, outcome)
    }

    fn make_fact_with_timestamp(ctx: &str, outcome: &str, ts: &str, idx: i64) -> Fact {
        let mut f = Fact::new(ctx, outcome);
        f.timestamp = ts.to_string();
        f.metadata
            .insert("temporal_index".into(), serde_json::json!(idx));
        f
    }

    // -----------------------------------------------------------------------
    // extract_entity_ids
    // -----------------------------------------------------------------------

    #[test]
    fn extract_entity_ids_finds_structured_ids() {
        let ids = extract_entity_ids("Check INC-2024-001 and CVE-2023-44228 please");
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"INC-2024-001".to_string()));
        assert!(ids.contains(&"CVE-2023-44228".to_string()));
    }

    #[test]
    fn extract_entity_ids_empty_on_no_match() {
        assert!(extract_entity_ids("no structured ids here").is_empty());
    }

    #[test]
    fn extract_entity_ids_rejects_single_char_prefix() {
        assert!(extract_entity_ids("X-2024-001").is_empty());
    }

    #[test]
    fn extract_entity_ids_rejects_long_prefix() {
        assert!(extract_entity_ids("TOOLONG-2024-001").is_empty());
    }

    // -----------------------------------------------------------------------
    // summarize_old_facts
    // -----------------------------------------------------------------------

    #[test]
    fn summarize_empty_returns_empty() {
        assert!(summarize_old_facts(&[], "entity").is_empty());
    }

    #[test]
    fn summarize_small_group_returns_verbatim() {
        let facts = vec![
            make_fact("Project:Alpha", "fact one"),
            make_fact("Project:Alpha", "fact two"),
        ];
        let result = summarize_old_facts(&facts, "entity");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn summarize_large_group_produces_summary() {
        let facts: Vec<Fact> = (0..5)
            .map(|i| make_fact("Project:Alpha", &format!("fact number {i}")))
            .collect();
        let result = summarize_old_facts(&facts, "entity");
        assert_eq!(result.len(), 1);
        assert!(result[0].context.contains("SUMMARY"));
        assert!(result[0].outcome.contains("5 facts"));
        assert!(result[0].tags.contains(&"summary".to_string()));
        assert!(result[0].tags.contains(&"entity".to_string()));
    }

    #[test]
    fn summarize_topic_level_groups_by_prefix() {
        let facts = vec![
            make_fact("Security:Auth", "fact 1"),
            make_fact("Security:Crypto", "fact 2"),
            make_fact("Security:Network", "fact 3"),
            make_fact("Performance.Latency", "fact 4"),
            make_fact("Performance.Throughput", "fact 5"),
            make_fact("Performance.Cache", "fact 6"),
        ];
        let result = summarize_old_facts(&facts, "topic");
        let summaries: Vec<_> = result
            .iter()
            .filter(|f| f.context.contains("SUMMARY"))
            .collect();
        assert_eq!(summaries.len(), 2);
    }

    #[test]
    fn summarize_skips_empty_outcomes() {
        let mut facts: Vec<Fact> = (0..4)
            .map(|i| make_fact("Group:A", &format!("fact {i}")))
            .collect();
        facts.push(make_fact("Group:A", ""));
        let result = summarize_old_facts(&facts, "entity");
        assert_eq!(result.len(), 1);
        assert!(!result[0].outcome.contains(";;"));
    }

    // -----------------------------------------------------------------------
    // tiered_retrieval
    // -----------------------------------------------------------------------

    #[test]
    fn tiered_small_set_returns_all() {
        let facts: Vec<Fact> = (0..5)
            .map(|i| make_fact_with_timestamp("ctx", &format!("fact {i}"), "2024-01-01", i))
            .collect();
        let result = tiered_retrieval("general question", &facts);
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn tiered_preserves_exact_id_matches() {
        let mut facts: Vec<Fact> = (0..10)
            .map(|i| {
                make_fact_with_timestamp("general", &format!("fact {i}"), "2024-01-01", i)
            })
            .collect();
        facts.push(make_fact("incident", "INC-2024-001 was critical"));
        let result = tiered_retrieval("What about INC-2024-001?", &facts);
        assert!(result.iter().any(|f| f.outcome.contains("INC-2024-001")));
    }

    // -----------------------------------------------------------------------
    // simple_retrieval with pre_snapshot
    // -----------------------------------------------------------------------

    #[test]
    fn simple_retrieval_small_snapshot_exhaustive() {
        let mem = MockMemory::new(vec![]);
        let snapshot: Vec<Fact> =
            (0..5).map(|i| make_fact("ctx", &format!("f{i}"))).collect();
        let (result, exhaustive) =
            simple_retrieval(&mem, "question", false, Some(&snapshot));
        assert!(exhaustive);
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn simple_retrieval_force_verbatim() {
        let mem = MockMemory::new(vec![]);
        let snapshot: Vec<Fact> = (0..2000)
            .map(|i| make_fact("ctx", &format!("f{i}")))
            .collect();
        let (result, exhaustive) =
            simple_retrieval(&mem, "question", true, Some(&snapshot));
        assert!(exhaustive);
        assert_eq!(result.len(), 2000);
    }

    #[test]
    fn simple_retrieval_large_snapshot_tiered() {
        let mem = MockMemory::new(vec![]);
        let snapshot: Vec<Fact> = (0..2000)
            .map(|i| make_fact("ctx", &format!("f{i}")))
            .collect();
        let (result, exhaustive) =
            simple_retrieval(&mem, "question", false, Some(&snapshot));
        assert!(!exhaustive);
        assert!(result.len() < 2000);
    }

    // -----------------------------------------------------------------------
    // simple_retrieval without pre_snapshot
    // -----------------------------------------------------------------------

    #[test]
    fn simple_retrieval_small_kb_exhaustive() {
        let facts: Vec<Fact> = (0..10)
            .map(|i| make_fact("ctx", &format!("f{i}")))
            .collect();
        let mem = MockMemory::new(facts).with_stats(MemoryStatistics {
            total_experiences: Some(10),
            ..Default::default()
        });
        let (result, exhaustive) = simple_retrieval(&mem, "question", false, None);
        assert!(exhaustive);
        assert_eq!(result.len(), 10);
    }

    // -----------------------------------------------------------------------
    // entity_retrieval
    // -----------------------------------------------------------------------

    #[test]
    fn entity_retrieval_requires_hierarchical() {
        let mem = MockMemory::new(vec![make_fact("ctx", "data")]);
        let result = entity_retrieval(&mem, "What about Sarah Chen?", false);
        assert!(result.is_empty());
    }

    #[test]
    fn entity_retrieval_extracts_proper_nouns() {
        let facts = vec![
            make_fact("Sarah Chen", "lead engineer"),
            make_fact("Bob Smith", "product manager"),
            make_fact("general", "unrelated fact"),
        ];
        let mem = MockMemory::new(facts).with_hierarchical();
        let result = entity_retrieval(&mem, "Tell me about Sarah Chen", false);
        assert!(!result.is_empty());
        assert!(result.iter().any(|f| f.context.contains("Sarah")));
    }

    #[test]
    fn entity_retrieval_handles_possessives() {
        let facts = vec![make_fact("Fatima", "completed the project")];
        let mem = MockMemory::new(facts).with_hierarchical();
        let result = entity_retrieval(&mem, "What is Fatima's role?", false);
        assert!(!result.is_empty());
    }

    #[test]
    fn entity_retrieval_fallback_capitalized_words() {
        let facts = vec![make_fact("kubernetes", "container orchestration")];
        let mem = MockMemory::new(facts).with_hierarchical();
        let result = entity_retrieval(&mem, "Kubernetes deployment", false);
        assert!(!result.is_empty());
    }

    #[test]
    fn entity_retrieval_no_candidates_returns_empty() {
        let facts = vec![make_fact("ctx", "data")];
        let mem = MockMemory::new(facts).with_hierarchical();
        let result = entity_retrieval(&mem, "what is this about?", false);
        assert!(result.is_empty());
    }

    // -----------------------------------------------------------------------
    // concept_retrieval
    // -----------------------------------------------------------------------

    #[test]
    fn concept_retrieval_requires_hierarchical() {
        let mem = MockMemory::new(vec![make_fact("ctx", "data")]);
        let result = concept_retrieval(&mem, "authentication patterns", false);
        assert!(result.is_empty());
    }

    #[test]
    fn concept_retrieval_extracts_keywords() {
        let facts = vec![
            make_fact("authentication", "JWT tokens used for auth"),
            make_fact("deployment", "kubernetes cluster"),
        ];
        let mem = MockMemory::new(facts).with_hierarchical();
        let result =
            concept_retrieval(&mem, "How does authentication work?", false);
        assert!(!result.is_empty());
    }

    #[test]
    fn concept_retrieval_filters_stop_words() {
        let facts = vec![make_fact("security", "important data")];
        let mem = MockMemory::new(facts).with_hierarchical();
        let result = concept_retrieval(&mem, "the is a", false);
        assert!(result.is_empty());
    }

    #[test]
    fn concept_retrieval_deduplicates() {
        let facts = vec![
            make_fact("auth", "login system"),
            make_fact("auth", "login system"),
        ];
        let mem = MockMemory::new(facts).with_hierarchical();
        let result =
            concept_retrieval(&mem, "authentication login", false);
        let keys: HashSet<String> =
            result.iter().map(|f| f.dedup_key()).collect();
        assert_eq!(keys.len(), result.len());
    }

    // -----------------------------------------------------------------------
    // estimate_total_fact_count
    // -----------------------------------------------------------------------

    #[test]
    fn estimate_with_snapshot() {
        let mem = MockMemory::new(vec![]);
        let snap = vec![make_fact("a", "b"), make_fact("c", "d")];
        assert_eq!(estimate_total_fact_count(&mem, Some(&snap)), Some(2));
    }

    #[test]
    fn estimate_with_stats() {
        let mem = MockMemory::new(vec![]).with_stats(MemoryStatistics {
            total_experiences: Some(42),
            ..Default::default()
        });
        assert_eq!(estimate_total_fact_count(&mem, None), Some(42));
    }

    #[test]
    fn estimate_no_stats() {
        let mem = MockMemory::new(vec![]);
        assert_eq!(estimate_total_fact_count(&mem, None), None);
    }

    // -----------------------------------------------------------------------
    // ENTITY_ID_RE regex edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn entity_id_regex_boundary() {
        let ids = extract_entity_ids("prefixINC-2024-001suffix");
        assert!(ids.is_empty());
    }

    #[test]
    fn entity_id_regex_multiple() {
        let ids =
            extract_entity_ids("INC-2024-001 SEC-2023-42 BUG-9999-99999");
        // All three match: 2-5 uppercase prefix, 4-digit year, 2-5 digit suffix
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"INC-2024-001".to_string()));
        assert!(ids.contains(&"SEC-2023-42".to_string()));
        assert!(ids.contains(&"BUG-9999-99999".to_string()));
    }
}
