use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::hash_key;

/// A fact stored in a shard.
///
/// Lighter than `HiveFact` — no graph edges, no embedding storage.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardFact {
    pub fact_id: String,
    pub content: String,
    #[serde(default)]
    pub concept: String,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default)]
    pub source_agent: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub created_at: f64,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub ring_position: u32,
}

fn default_confidence() -> f64 {
    0.8
}

impl ShardFact {
    /// Create a minimal fact with the given ID and content.
    pub fn new(fact_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            fact_id: fact_id.into(),
            content: content.into(),
            concept: String::new(),
            confidence: 0.8,
            source_agent: String::new(),
            tags: Vec::new(),
            created_at: 0.0,
            metadata: HashMap::new(),
            ring_position: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Stop words for keyword search
// ---------------------------------------------------------------------------

const SEARCH_STOP_WORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "what", "how", "does", "do", "and", "or", "of",
    "in", "to", "for", "with", "on", "at", "by", "from", "that", "this", "it", "as", "be",
    "been", "has", "have", "had", "will", "would", "could", "should", "did", "which", "who",
    "when", "where", "why", "any", "some", "all", "both", "each", "few", "more", "most", "other",
    "such", "into", "through", "during", "before", "after", "than", "then", "these", "those",
    "there", "their", "they", "its",
];

/// Strip leading/trailing punctuation from a word (mirrors Python's `str.strip`).
fn strip_punctuation(word: &str) -> &str {
    let s = word.trim_start_matches(|c: char| "?.,!;:'\"()[]".contains(c));
    s.trim_end_matches(|c: char| "?.,!;:'\"()[]".contains(c))
}

/// Score a fact against a set of search terms and bigrams.
fn score_fact(
    fact: &ShardFact,
    terms: &HashSet<String>,
    bigrams: &HashSet<(String, String)>,
) -> f64 {
    let content_lower = fact.content.to_lowercase();
    let content_words: Vec<&str> = content_lower.split_whitespace().collect();
    let content_word_set: HashSet<&str> = content_words.iter().copied().collect();

    let mut hits: f64 = 0.0;
    for term in terms {
        let weight = if term.chars().any(|c| c.is_ascii_digit()) {
            5.0
        } else {
            1.0
        };
        if content_lower.contains(term.as_str()) {
            hits += weight;
        } else if term.len() >= 4 {
            let partial = content_word_set.iter().any(|w| {
                w.len() >= 4 && (w.starts_with(term.as_str()) || term.starts_with(w))
            });
            if partial {
                hits += weight * 0.5;
            }
        }
    }

    if hits <= 0.0 {
        return 0.0;
    }

    // Bigram bonus: reward facts that share consecutive-word matches.
    let fact_bigrams: HashSet<(&str, &str)> = content_words
        .windows(2)
        .map(|w| (w[0], w[1]))
        .collect();
    let bigram_hits = bigrams
        .iter()
        .filter(|(a, b)| fact_bigrams.contains(&(a.as_str(), b.as_str())))
        .count();
    let bigram_bonus = bigram_hits as f64 * 0.3;

    hits + bigram_bonus + fact.confidence * 0.01
}

/// Lightweight per-agent fact storage.
///
/// Each agent has one `ShardStore` holding its portion of the DHT.
/// Facts stored here are those assigned to this agent by the hash ring.
#[derive(Debug)]
pub struct ShardStore {
    agent_id: String,
    facts: HashMap<String, ShardFact>,
    content_index: HashMap<String, String>,
}

impl ShardStore {
    /// Create an empty store for the given agent.
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            facts: HashMap::new(),
            content_index: HashMap::new(),
        }
    }

    /// Return the owning agent ID.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Store a fact.  Returns `false` if the content is a duplicate.
    pub fn store(&mut self, fact: ShardFact) -> bool {
        let content_hash = format!("{:x}", hash_key(&fact.content));
        if self.content_index.contains_key(&content_hash) {
            return false;
        }
        self.content_index
            .insert(content_hash, fact.fact_id.clone());
        self.facts.insert(fact.fact_id.clone(), fact);
        true
    }

    /// Get a fact by ID.
    pub fn get(&self, fact_id: &str) -> Option<&ShardFact> {
        self.facts.get(fact_id)
    }

    /// Keyword search with substring matching and bigram overlap scoring.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&ShardFact> {
        let query_lower = query.to_lowercase();
        let raw_words: Vec<String> = query_lower
            .split_whitespace()
            .map(|w| strip_punctuation(w).to_string())
            .filter(|w| !w.is_empty())
            .collect();

        let stop: HashSet<&str> = SEARCH_STOP_WORDS.iter().copied().collect();
        let mut terms: HashSet<String> = raw_words
            .iter()
            .filter(|w| !stop.contains(w.as_str()) && w.len() > 1)
            .cloned()
            .collect();
        if terms.is_empty() {
            terms = raw_words.iter().cloned().collect();
        }

        let bigrams: HashSet<(String, String)> = raw_words
            .windows(2)
            .map(|w| (w[0].clone(), w[1].clone()))
            .collect();

        let mut scored: Vec<(f64, &ShardFact)> = self
            .facts
            .values()
            .filter(|f| !f.tags.contains(&"retracted".to_string()))
            .filter_map(|f| {
                let s = score_fact(f, &terms, &bigrams);
                if s > 0.0 { Some((s, f)) } else { None }
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(limit).map(|(_, f)| f).collect()
    }

    /// Get all fact IDs in this shard.
    pub fn fact_ids(&self) -> HashSet<String> {
        self.facts.keys().cloned().collect()
    }

    /// Get all facts in this shard.
    pub fn all_facts(&self) -> Vec<&ShardFact> {
        self.facts.values().collect()
    }

    /// Return the number of facts stored.
    pub fn fact_count(&self) -> usize {
        self.facts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fact(id: &str, content: &str) -> ShardFact {
        ShardFact::new(id, content)
    }

    #[test]
    fn store_and_get() {
        let mut store = ShardStore::new("agent-1");
        let fact = sample_fact("f1", "Rust is a systems language");
        assert!(store.store(fact));
        assert!(store.get("f1").is_some());
    }

    #[test]
    fn duplicate_content_rejected() {
        let mut store = ShardStore::new("agent-1");
        assert!(store.store(sample_fact("f1", "same content")));
        assert!(!store.store(sample_fact("f2", "same content")));
        assert_eq!(store.fact_count(), 1);
    }

    #[test]
    fn search_finds_matching_facts() {
        let mut store = ShardStore::new("a");
        store.store(sample_fact("f1", "Rust is a systems programming language"));
        store.store(sample_fact("f2", "Python is an interpreted language"));
        store.store(sample_fact("f3", "Completely unrelated topic here"));

        let results = store.search("Rust programming", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].fact_id, "f1");
    }

    #[test]
    fn search_skips_retracted() {
        let mut store = ShardStore::new("a");
        let mut fact = sample_fact("f1", "Important fact about Rust");
        fact.tags.push("retracted".to_string());
        store.store(fact);

        let results = store.search("Rust", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn search_weights_identifiers_higher() {
        let mut store = ShardStore::new("a");
        store.store(sample_fact("f1", "Incident INC-2024-001 caused outage"));
        store.store(sample_fact("f2", "Some general incident discussion"));

        let results = store.search("INC-2024-001", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].fact_id, "f1");
    }

    #[test]
    fn fact_ids_and_all_facts() {
        let mut store = ShardStore::new("a");
        store.store(sample_fact("f1", "one"));
        store.store(sample_fact("f2", "two"));
        assert_eq!(store.fact_ids().len(), 2);
        assert_eq!(store.all_facts().len(), 2);
    }
}
