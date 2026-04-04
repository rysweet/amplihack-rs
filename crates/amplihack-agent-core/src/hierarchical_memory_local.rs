//! Local hierarchical memory — in-memory implementation.
//!
//! Port of Python `_hierarchical_memory_local.py` — the `HierarchicalMemoryLocal`
//! struct backed by in-memory `Vec` storage instead of Kuzu.

use std::collections::HashMap;

use crate::hierarchical_memory_types::{
    KnowledgeEdge, KnowledgeNode, KnowledgeSubgraph, MemoryCategory, StoreKnowledgeParams,
};
use crate::similarity::compute_word_similarity;

/// In-memory hierarchical memory implementation.
///
/// Faithful port of the Python `HierarchicalMemory` class, using in-memory
/// `Vec` storage instead of Kuzu for zero-dependency operation.
pub struct HierarchicalMemoryLocal {
    agent_name: String,
    pub(crate) nodes: Vec<KnowledgeNode>,
    pub(crate) edges: Vec<KnowledgeEdge>,
    episodes: Vec<KnowledgeNode>,
    next_id: u64,
}

impl HierarchicalMemoryLocal {
    pub fn new(agent_name: impl Into<String>) -> Self {
        let name = agent_name.into();
        assert!(!name.trim().is_empty(), "agent_name cannot be empty");
        Self {
            agent_name: name,
            nodes: Vec::new(),
            edges: Vec::new(),
            episodes: Vec::new(),
            next_id: 1,
        }
    }

    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    /// Store a knowledge node, auto-creating similarity edges.
    pub fn store_knowledge(&mut self, p: StoreKnowledgeParams<'_>) -> String {
        let node_id = format!("node-{}", self.next_id);
        self.next_id += 1;
        let mut metadata: HashMap<String, serde_json::Value> = HashMap::new();
        if let Some(tm) = p.temporal_metadata {
            metadata.extend(tm.clone());
        }
        let node = KnowledgeNode {
            node_id: node_id.clone(),
            category: p.category,
            content: p.content.trim().to_string(),
            concept: p.concept.trim().to_string(),
            confidence: p.confidence,
            source_id: p.source_id.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            tags: p.tags.to_vec(),
            metadata,
        };
        let new_edges: Vec<KnowledgeEdge> = self
            .nodes
            .iter()
            .filter_map(|existing| {
                let sim = compute_word_similarity(&node.content, &existing.content);
                if sim >= 0.3 {
                    Some(KnowledgeEdge {
                        source_id: node.node_id.clone(),
                        target_id: existing.node_id.clone(),
                        relationship: "SIMILAR_TO".into(),
                        weight: sim,
                        metadata: HashMap::new(),
                    })
                } else {
                    None
                }
            })
            .collect();
        self.nodes.push(node);
        self.edges.extend(new_edges);
        node_id
    }

    /// Store an episode (raw source content).
    pub fn store_episode(&mut self, content: &str, source_label: &str) -> String {
        let eid = format!("ep-{}", self.next_id);
        self.next_id += 1;
        self.episodes.push(KnowledgeNode {
            node_id: eid.clone(),
            category: MemoryCategory::Episodic,
            content: content.to_string(),
            concept: source_label.to_string(),
            confidence: 1.0,
            source_id: String::new(),
            created_at: chrono::Utc::now().to_rfc3339(),
            tags: Vec::new(),
            metadata: HashMap::new(),
        });
        eid
    }

    /// Retrieve a subgraph relevant to the query.
    pub fn retrieve_subgraph(&self, query: &str, max_nodes: usize) -> KnowledgeSubgraph {
        if query.trim().is_empty() {
            return KnowledgeSubgraph::new(query);
        }
        let lower = query.to_lowercase();
        let keywords: Vec<&str> = lower.split_whitespace().filter(|w| w.len() > 2).collect();
        let mut scored: Vec<(f64, &KnowledgeNode)> = self
            .nodes
            .iter()
            .filter_map(|n| {
                let text = format!("{} {}", n.concept, n.content).to_lowercase();
                let hits = keywords.iter().filter(|k| text.contains(**k)).count();
                if hits > 0 {
                    Some((hits as f64 * n.confidence, n))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let nodes: Vec<KnowledgeNode> = scored
            .into_iter()
            .take(max_nodes)
            .map(|(_, n)| n.clone())
            .collect();
        let ids: std::collections::HashSet<&str> =
            nodes.iter().map(|n| n.node_id.as_str()).collect();
        let edges: Vec<KnowledgeEdge> = self
            .edges
            .iter()
            .filter(|e| ids.contains(e.source_id.as_str()) || ids.contains(e.target_id.as_str()))
            .cloned()
            .collect();
        KnowledgeSubgraph {
            nodes,
            edges,
            query: query.to_string(),
        }
    }

    pub fn get_all_knowledge(&self, limit: usize) -> Vec<KnowledgeNode> {
        self.nodes.iter().rev().take(limit).cloned().collect()
    }

    pub fn search_by_concept(&self, keywords: &[String], limit: usize) -> Vec<KnowledgeNode> {
        let kw: Vec<String> = keywords.iter().map(|k| k.to_lowercase()).collect();
        self.nodes
            .iter()
            .filter(|n| {
                let t = format!("{} {}", n.concept, n.content).to_lowercase();
                kw.iter().any(|k| t.contains(k.as_str()))
            })
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn retrieve_by_entity(&self, entity: &str, limit: usize) -> Vec<KnowledgeNode> {
        let lower = entity.to_lowercase();
        self.nodes
            .iter()
            .filter(|n| {
                n.concept.to_lowercase().contains(&lower)
                    || n.content.to_lowercase().contains(&lower)
            })
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn execute_aggregation(
        &self,
        query_type: &str,
        entity_filter: &str,
    ) -> HashMap<String, serde_json::Value> {
        let mut r = HashMap::new();
        match query_type {
            "count_entities" => {
                let count = if entity_filter.is_empty() {
                    self.nodes.len()
                } else {
                    let lf = entity_filter.to_lowercase();
                    self.nodes
                        .iter()
                        .filter(|n| n.concept.to_lowercase().contains(&lf))
                        .count()
                };
                r.insert("count".into(), serde_json::json!(count));
            }
            "list_concepts" => {
                let mut c: Vec<String> = self.nodes.iter().map(|n| n.concept.clone()).collect();
                c.sort();
                c.dedup();
                r.insert("concepts".into(), serde_json::json!(c));
            }
            _ => {
                r.insert(
                    "error".into(),
                    serde_json::json!(format!("unsupported: {query_type}")),
                );
            }
        }
        r
    }

    pub fn get_statistics(&self) -> HashMap<String, serde_json::Value> {
        HashMap::from([
            ("total_nodes".into(), serde_json::json!(self.nodes.len())),
            ("total_edges".into(), serde_json::json!(self.edges.len())),
            (
                "total_episodes".into(),
                serde_json::json!(self.episodes.len()),
            ),
        ])
    }

    pub fn export_to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "agent_name": self.agent_name, "nodes": self.nodes,
            "edges": self.edges, "episodes": self.episodes,
            "statistics": { "total_nodes": self.nodes.len(),
                "total_edges": self.edges.len(), "total_episodes": self.episodes.len() }
        })
    }

    pub fn import_from_json(
        &mut self,
        data: &serde_json::Value,
        merge: bool,
    ) -> HashMap<String, serde_json::Value> {
        if !merge {
            self.nodes.clear();
            self.edges.clear();
            self.episodes.clear();
        }
        let (mut in_n, mut in_e) = (0usize, 0usize);
        if let Some(arr) = data.get("nodes").and_then(|v| v.as_array()) {
            for v in arr {
                if let Ok(n) = serde_json::from_value::<KnowledgeNode>(v.clone()) {
                    self.nodes.push(n);
                    in_n += 1;
                }
            }
        }
        if let Some(arr) = data.get("edges").and_then(|v| v.as_array()) {
            for v in arr {
                if let Ok(e) = serde_json::from_value::<KnowledgeEdge>(v.clone()) {
                    self.edges.push(e);
                    in_e += 1;
                }
            }
        }
        HashMap::from([
            ("imported_nodes".into(), serde_json::json!(in_n)),
            ("imported_edges".into(), serde_json::json!(in_e)),
            ("merge".into(), serde_json::json!(merge)),
        ])
    }

    pub fn flush_memory(&self) {}
    pub fn close(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(mem: &mut HierarchicalMemoryLocal, content: &str, concept: &str, conf: f64) -> String {
        mem.store_knowledge(StoreKnowledgeParams {
            content,
            concept,
            confidence: conf,
            category: MemoryCategory::Semantic,
            source_id: "",
            tags: &[],
            temporal_metadata: None,
        })
    }

    #[test]
    fn store_retrieve_and_edges() {
        let mut mem = HierarchicalMemoryLocal::new("test");
        let id = store(&mut mem, "Cells are the basic unit of life", "Biology", 0.9);
        assert!(id.starts_with("node-"));
        assert_eq!(mem.get_all_knowledge(10).len(), 1);
        store(
            &mut mem,
            "photosynthesis converts light energy",
            "Biology",
            0.9,
        );
        store(
            &mut mem,
            "photosynthesis produces oxygen from light",
            "Biology",
            0.8,
        );
        assert!(!mem.edges.is_empty());
    }

    #[test]
    fn subgraph_search_entity_concept() {
        let mut mem = HierarchicalMemoryLocal::new("test");
        store(&mut mem, "photosynthesis", "Biology", 0.9);
        store(&mut mem, "quantum mechanics", "Physics", 0.8);
        let sg = mem.retrieve_subgraph("photosynthesis", 10);
        assert_eq!(sg.nodes.len(), 1);
        assert!(mem.retrieve_subgraph("", 10).nodes.is_empty());
        store(&mut mem, "DNA replication", "Genetics", 0.9);
        assert_eq!(mem.search_by_concept(&["genetics".into()], 10).len(), 1);
        store(&mut mem, "Mars has polar ice caps", "Mars", 0.9);
        assert_eq!(mem.retrieve_by_entity("mars", 10).len(), 1);
    }

    #[test]
    fn aggregation_episode_stats() {
        let mut mem = HierarchicalMemoryLocal::new("test");
        store(&mut mem, "f1", "Bio", 0.9);
        store(&mut mem, "f2", "Phys", 0.8);
        assert_eq!(
            mem.execute_aggregation("count_entities", "")["count"],
            serde_json::json!(2)
        );
        assert_eq!(
            mem.execute_aggregation("list_concepts", "")["concepts"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let eid = mem.store_episode("raw", "src");
        assert!(eid.starts_with("ep-"));
        assert_eq!(mem.get_statistics()["total_episodes"], serde_json::json!(1));
    }

    #[test]
    fn export_import_roundtrip_and_merge() {
        let mut mem = HierarchicalMemoryLocal::new("test");
        store(&mut mem, "fact", "Topic", 0.9);
        let exported = mem.export_to_json();
        let mut m2 = HierarchicalMemoryLocal::new("a2");
        let s = m2.import_from_json(&exported, false);
        assert_eq!(s["imported_nodes"], serde_json::json!(1));
        assert_eq!(m2.get_all_knowledge(10).len(), 1);
        // merge mode
        let mut m3 = HierarchicalMemoryLocal::new("a3");
        store(&mut m3, "existing", "X", 0.9);
        let data = serde_json::json!({"nodes":[{"node_id":"i","category":"semantic","content":"n","concept":"N","confidence":0.8}],"edges":[]});
        m3.import_from_json(&data, true);
        assert_eq!(m3.get_all_knowledge(50).len(), 2);
    }

    #[test]
    #[should_panic(expected = "agent_name cannot be empty")]
    fn empty_name() {
        HierarchicalMemoryLocal::new("");
    }
}
