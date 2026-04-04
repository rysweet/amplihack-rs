//! [`DistributedCognitiveMemory`] — transparent local+hive memory proxy.

use super::HIVE_SEARCH_MULTIPLIER;
use super::graph::DistributedHiveGraph;
use super::merge::{extract_content, merge_fact_lists, relevance_score};
use crate::models::{FACT_ID_HEX_LENGTH, HiveFact};
use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

/// Transparent proxy wrapping local storage and a distributed hive graph.
pub struct DistributedCognitiveMemory {
    local_facts: Vec<serde_json::Value>,
    hive: DistributedHiveGraph,
    agent_name: String,
    quality_threshold: f64,
}

impl DistributedCognitiveMemory {
    pub fn new(
        hive: DistributedHiveGraph,
        agent_name: impl Into<String>,
        quality_threshold: f64,
    ) -> Self {
        Self {
            local_facts: Vec::new(),
            hive,
            agent_name: agent_name.into(),
            quality_threshold,
        }
    }

    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }
    pub fn local_fact_count(&self) -> usize {
        self.local_facts.len()
    }
    pub fn hive(&self) -> &DistributedHiveGraph {
        &self.hive
    }
    pub fn hive_mut(&mut self) -> &mut DistributedHiveGraph {
        &mut self.hive
    }

    /// Store a fact locally and auto-promote to the hive if quality passes.
    pub fn store_fact(
        &mut self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: Vec<String>,
    ) -> String {
        let fact_id = format!(
            "{:0>width$}",
            &Uuid::new_v4().to_string().replace('-', "")[..FACT_ID_HEX_LENGTH],
            width = FACT_ID_HEX_LENGTH
        );
        self.local_facts.push(serde_json::json!({
            "experience_id": fact_id, "outcome": content, "context": concept,
            "confidence": confidence, "tags": tags, "timestamp": Utc::now().to_rfc3339(),
        }));
        self.auto_promote(concept, content, confidence, &tags);
        fact_id
    }

    /// Search facts across local + hive with relevance merge.
    pub fn search_facts(
        &self,
        query: &str,
        limit: usize,
        _min_confidence: f64,
    ) -> Vec<serde_json::Value> {
        let expanded = limit * HIVE_SEARCH_MULTIPLIER;
        let local = self.local_search_facts(query, expanded);
        let hive = self.query_hive(query, expanded);
        merge_fact_lists(&local, &hive, limit, query)
    }

    pub fn get_all_facts(&self, limit: usize) -> Vec<serde_json::Value> {
        let expanded = limit * HIVE_SEARCH_MULTIPLIER;
        let local = self.local_get_all_facts(expanded);
        let hive = self.get_all_hive_facts(expanded);
        merge_fact_lists(&local, &hive, limit, "")
    }

    pub fn search_by_concept(&self, keywords: &[&str], limit: usize) -> Vec<serde_json::Value> {
        let query = keywords
            .iter()
            .take(super::QUERY_KEYWORD_LIMIT)
            .copied()
            .collect::<Vec<&str>>()
            .join(" ");
        self.search_facts(&query, limit, 0.0)
    }

    pub fn retrieve_by_entity(&self, entity_name: &str, limit: usize) -> Vec<serde_json::Value> {
        let local = self.local_search_facts(entity_name, limit);
        let hive = self.query_hive(entity_name, limit);
        merge_fact_lists(&local, &hive, limit, entity_name)
    }

    pub fn local_search_facts(&self, query: &str, limit: usize) -> Vec<serde_json::Value> {
        let mut scored: Vec<(f64, &serde_json::Value)> = self
            .local_facts
            .iter()
            .map(|f| (relevance_score(&extract_content(f), query), f))
            .filter(|(s, _)| *s > 0.0)
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, f)| f.clone())
            .collect()
    }

    pub fn local_get_all_facts(&self, limit: usize) -> Vec<serde_json::Value> {
        self.local_facts.iter().take(limit).cloned().collect()
    }

    fn query_hive(&self, query: &str, limit: usize) -> Vec<serde_json::Value> {
        self.hive
            .query_facts(query, limit)
            .into_iter()
            .map(|sf| shard_fact_to_dict(&sf))
            .collect()
    }

    fn get_all_hive_facts(&self, limit: usize) -> Vec<serde_json::Value> {
        self.hive
            .query_facts("", limit)
            .into_iter()
            .map(|sf| shard_fact_to_dict(&sf))
            .collect()
    }

    fn auto_promote(&mut self, concept: &str, content: &str, confidence: f64, tags: &[String]) {
        if self.quality_threshold > 0.0 {
            let quality = crate::quality::score_content_quality(content, concept);
            if quality < self.quality_threshold {
                return;
            }
        }
        let fact = HiveFact {
            fact_id: format!(
                "{:0>width$}",
                &Uuid::new_v4().to_string().replace('-', "")[..FACT_ID_HEX_LENGTH],
                width = FACT_ID_HEX_LENGTH
            ),
            concept: concept.to_string(),
            content: content.to_string(),
            confidence,
            source_id: self.agent_name.clone(),
            tags: tags.to_vec(),
            created_at: Utc::now(),
            status: "promoted".into(),
            metadata: HashMap::new(),
        };
        self.hive.promote_fact(&self.agent_name, fact);
    }
}

fn shard_fact_to_dict(fact: &crate::dht::ShardFact) -> serde_json::Value {
    let mut meta = serde_json::Map::new();
    for tag in &fact.tags {
        if let Some(date) = tag.strip_prefix("date:") {
            meta.insert("date".into(), serde_json::json!(date));
        } else if let Some(time) = tag.strip_prefix("time:") {
            meta.insert("time".into(), serde_json::json!(time));
        }
    }
    serde_json::json!({
        "experience_id": fact.fact_id, "outcome": fact.content, "context": fact.concept,
        "confidence": fact.confidence, "source": fact.source_agent,
        "tags": fact.tags, "metadata": meta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DEFAULT_TRUST_SCORE;

    fn make_memory() -> DistributedCognitiveMemory {
        let mut hive = DistributedHiveGraph::new();
        hive.register_agent("test-agent", "security", DEFAULT_TRUST_SCORE);
        DistributedCognitiveMemory::new(hive, "test-agent", 0.0)
    }

    #[test]
    fn store_and_search() {
        let mut mem = make_memory();
        mem.store_fact("security", "SQL injection is dangerous", 0.9, vec![]);
        assert!(!mem.search_facts("SQL injection", 10, 0.0).is_empty());
    }

    #[test]
    fn store_promotes_to_hive() {
        let mut mem = make_memory();
        mem.store_fact("topic", "Rust memory safety is great", 0.8, vec![]);
        assert!(!mem.hive().query_facts("Rust memory", 10).is_empty());
    }

    #[test]
    fn get_all_facts() {
        let mut mem = make_memory();
        mem.store_fact("a", "fact one", 0.9, vec![]);
        mem.store_fact("b", "fact two", 0.8, vec![]);
        assert_eq!(mem.get_all_facts(10).len(), 2);
    }

    #[test]
    fn search_by_concept() {
        let mut mem = make_memory();
        mem.store_fact("rust", "Rust ownership model", 0.9, vec![]);
        assert!(!mem.search_by_concept(&["rust", "ownership"], 10).is_empty());
    }

    #[test]
    fn local_only_search() {
        let mut mem = make_memory();
        mem.store_fact("topic", "local only fact", 0.9, vec![]);
        assert!(!mem.local_search_facts("local", 10).is_empty());
    }

    #[test]
    fn local_fact_count() {
        let mut mem = make_memory();
        assert_eq!(mem.local_fact_count(), 0);
        mem.store_fact("c", "content", 0.5, vec![]);
        assert_eq!(mem.local_fact_count(), 1);
    }

    #[test]
    fn quality_gate_blocks_low_quality() {
        let mut hive = DistributedHiveGraph::new();
        hive.register_agent("qa", "d", DEFAULT_TRUST_SCORE);
        let mut mem = DistributedCognitiveMemory::new(hive, "qa", 0.99);
        mem.store_fact("c", "x", 0.5, vec![]);
        assert!(mem.hive().query_facts("x", 10).is_empty());
    }

    #[test]
    fn shard_fact_to_dict_restores_metadata() {
        let fact = crate::dht::ShardFact {
            fact_id: "f1".into(),
            content: "content".into(),
            concept: "concept".into(),
            confidence: 0.9,
            source_agent: "agent".into(),
            tags: vec!["date:2024-01-01".into(), "time:12:00".into()],
            created_at: 0.0,
            metadata: HashMap::new(),
            ring_position: 0,
        };
        let dict = shard_fact_to_dict(&fact);
        assert_eq!(dict["metadata"]["date"], "2024-01-01");
        assert_eq!(dict["metadata"]["time"], "12:00");
    }

    #[test]
    fn retrieve_by_entity() {
        let mut mem = make_memory();
        mem.store_fact("server", "Apache server vulnerability", 0.9, vec![]);
        assert!(!mem.retrieve_by_entity("Apache", 10).is_empty());
    }
}
