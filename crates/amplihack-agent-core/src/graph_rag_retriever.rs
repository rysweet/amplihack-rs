//! Graph RAG retriever — keyword search, similarity expansion, provenance.
//!
//! Port of Python `graph_rag_retriever.py`. Provides structured methods for
//! knowledge graph traversal backed by [`HierarchicalMemoryLocal`]:
//! - `keyword_search` — find seed nodes via content/concept matching
//! - `similar_to_expand` — traverse similarity edges from a node
//! - `get_provenance` — find source episode for a semantic node
//! - `retrieve_subgraph` — full algorithm combining all methods

use std::collections::{HashMap, HashSet};

use crate::hierarchical_memory_local::HierarchicalMemoryLocal;
use crate::hierarchical_memory_types::{
    KnowledgeEdge, KnowledgeNode, KnowledgeSubgraph, MemoryCategory,
};

// ── GraphRagRetriever ────────────────────────────────────────────────────

/// Graph RAG retriever operating over a [`HierarchicalMemoryLocal`] instance.
///
/// Unlike the Python version which queries Kuzu directly, this Rust port
/// traverses the in-memory node/edge vectors — same algorithm, different
/// backend.
pub struct GraphRagRetriever<'a> {
    memory: &'a HierarchicalMemoryLocal,
    agent_name: String,
}

impl<'a> GraphRagRetriever<'a> {
    /// Create a new retriever scoped to the given agent.
    ///
    /// # Panics
    ///
    /// Panics if `agent_name` is empty.
    pub fn new(memory: &'a HierarchicalMemoryLocal, agent_name: &str) -> Self {
        assert!(!agent_name.trim().is_empty(), "agent_name cannot be empty");
        Self {
            memory,
            agent_name: agent_name.trim().to_string(),
        }
    }

    /// Agent name this retriever is scoped to.
    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    /// Search for nodes whose content or concept contains `keyword`.
    pub fn keyword_search(&self, keyword: &str, limit: usize) -> Vec<KnowledgeNode> {
        if keyword.trim().is_empty() {
            return Vec::new();
        }
        let lower = keyword.trim().to_lowercase();
        let mut matched: Vec<KnowledgeNode> = self.memory.nodes
            .iter()
            .filter(|n| {
                n.category == MemoryCategory::Semantic
                    && (n.content.to_lowercase().contains(&lower)
                        || n.concept.to_lowercase().contains(&lower))
            })
            .cloned()
            .collect();
        matched.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matched.truncate(limit);
        matched
    }

    /// Expand from a node via `SIMILAR_TO` edges with weight ≥ `min_similarity`.
    pub fn similar_to_expand(
        &self,
        node_id: &str,
        min_similarity: f64,
    ) -> Vec<(KnowledgeNode, f64)> {
        let node_map: HashMap<&str, &KnowledgeNode> =
            self.memory.nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();

        let mut results = Vec::new();
        for edge in self.memory_edges() {
            if edge.relationship != "SIMILAR_TO" || edge.weight < min_similarity {
                continue;
            }
            let target_id = if edge.source_id == node_id {
                &edge.target_id
            } else if edge.target_id == node_id {
                &edge.source_id
            } else {
                continue;
            };
            if let Some(target) = node_map.get(target_id.as_str()) {
                results.push(((*target).clone(), edge.weight));
            }
        }
        results.sort_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    /// Follow `DERIVES_FROM` edges to find source episodes.
    pub fn get_provenance(&self, node_id: &str) -> Vec<HashMap<String, serde_json::Value>> {
        let mut episodes = Vec::new();
        for edge in self.memory_edges() {
            if edge.source_id == node_id && edge.relationship == "DERIVES_FROM" {
                let mut ep = HashMap::new();
                ep.insert("episode_id".into(), serde_json::json!(edge.target_id));
                ep.insert(
                    "extraction_confidence".into(),
                    serde_json::json!(edge.weight),
                );
                episodes.push(ep);
            }
        }
        episodes
    }

    /// Full subgraph assembly: keyword search → seed expansion → dedup → rank.
    pub fn retrieve_subgraph(
        &self,
        query: &str,
        max_depth: usize,
        max_nodes: usize,
        min_similarity: f64,
    ) -> KnowledgeSubgraph {
        if query.trim().is_empty() {
            return KnowledgeSubgraph::new(query);
        }

        let mut seen: HashSet<String> = HashSet::new();
        let mut all_nodes: Vec<KnowledgeNode> = Vec::new();
        let mut all_edges: Vec<KnowledgeEdge> = Vec::new();

        // Step 1-2: keyword search for seeds.
        let keywords: Vec<&str> = query
            .split_whitespace()
            .map(|w| w.trim())
            .filter(|w| w.len() > 2)
            .collect();

        for kw in &keywords {
            let seed_nodes = self.keyword_search(kw, max_nodes);
            for node in seed_nodes {
                if !seen.contains(&node.node_id) {
                    seen.insert(node.node_id.clone());
                    all_nodes.push(node);
                }
            }
        }

        // Step 3: expand seeds via SIMILAR_TO.
        let mut seeds_to_expand: Vec<String> =
            all_nodes.iter().map(|n| n.node_id.clone()).collect();

        for _depth in 0..max_depth {
            if seen.len() >= max_nodes {
                break;
            }
            let mut next_seeds = Vec::new();
            for seed_id in &seeds_to_expand {
                if seen.len() >= max_nodes {
                    break;
                }
                let neighbors = self.similar_to_expand(seed_id, min_similarity);
                for (neighbor, weight) in neighbors {
                    all_edges.push(KnowledgeEdge {
                        source_id: seed_id.clone(),
                        target_id: neighbor.node_id.clone(),
                        relationship: "SIMILAR_TO".into(),
                        weight,
                        metadata: HashMap::new(),
                    });
                    if !seen.contains(&neighbor.node_id) && seen.len() < max_nodes {
                        seen.insert(neighbor.node_id.clone());
                        next_seeds.push(neighbor.node_id.clone());
                        all_nodes.push(neighbor);
                    }
                }
            }
            seeds_to_expand = next_seeds;
        }

        // Step 5: rank by confidence.
        all_nodes.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all_nodes.truncate(max_nodes);

        KnowledgeSubgraph {
            nodes: all_nodes,
            edges: all_edges,
            query: query.to_string(),
        }
    }

    fn memory_edges(&self) -> &[KnowledgeEdge] {
        &self.memory.edges
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hierarchical_memory_types::StoreKnowledgeParams;

    fn build_memory() -> HierarchicalMemoryLocal {
        let mut mem = HierarchicalMemoryLocal::new("test-agent");
        mem.store_knowledge(StoreKnowledgeParams {
            content: "photosynthesis converts light energy plants chlorophyll",
            concept: "Biology",
            confidence: 0.9,
            category: MemoryCategory::Semantic,
            source_id: "", tags: &[], temporal_metadata: None,
        });
        mem.store_knowledge(StoreKnowledgeParams {
            content: "photosynthesis light energy plants produce oxygen",
            concept: "Biology",
            confidence: 0.85,
            category: MemoryCategory::Semantic,
            source_id: "", tags: &[], temporal_metadata: None,
        });
        mem.store_knowledge(StoreKnowledgeParams {
            content: "quantum tunneling allows particles to pass barriers",
            concept: "Physics",
            confidence: 0.8,
            category: MemoryCategory::Semantic,
            source_id: "", tags: &[], temporal_metadata: None,
        });
        mem
    }

    #[test]
    fn keyword_search_basic() {
        let mem = build_memory();
        let retriever = GraphRagRetriever::new(&mem, "test-agent");

        let nodes = retriever.keyword_search("photosynthesis", 10);
        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().all(|n| n.concept == "Biology"));
    }

    #[test]
    fn keyword_search_empty() {
        let mem = build_memory();
        let retriever = GraphRagRetriever::new(&mem, "test-agent");
        assert!(retriever.keyword_search("", 10).is_empty());
    }

    #[test]
    fn keyword_search_no_match() {
        let mem = build_memory();
        let retriever = GraphRagRetriever::new(&mem, "test-agent");
        assert!(retriever.keyword_search("nonexistent", 10).is_empty());
    }

    #[test]
    fn similar_to_expand_finds_neighbors() {
        let mem = build_memory();
        let retriever = GraphRagRetriever::new(&mem, "test-agent");

        let seeds = retriever.keyword_search("photosynthesis", 1);
        assert!(!seeds.is_empty());
        let neighbors = retriever.similar_to_expand(&seeds[0].node_id, 0.1);
        // The two photosynthesis nodes should be similar.
        assert!(!neighbors.is_empty());
    }

    #[test]
    fn retrieve_subgraph_full() {
        let mem = build_memory();
        let retriever = GraphRagRetriever::new(&mem, "test-agent");

        let sg = retriever.retrieve_subgraph("photosynthesis light", 2, 20, 0.1);
        assert!(!sg.nodes.is_empty());
        assert_eq!(sg.query, "photosynthesis light");
    }

    #[test]
    fn retrieve_subgraph_empty_query() {
        let mem = build_memory();
        let retriever = GraphRagRetriever::new(&mem, "test-agent");

        let sg = retriever.retrieve_subgraph("", 2, 20, 0.3);
        assert!(sg.nodes.is_empty());
    }

    #[test]
    fn retrieve_subgraph_respects_max_nodes() {
        let mem = build_memory();
        let retriever = GraphRagRetriever::new(&mem, "test-agent");

        let sg = retriever.retrieve_subgraph("photosynthesis quantum", 2, 1, 0.1);
        assert!(sg.nodes.len() <= 1);
    }

    #[test]
    #[should_panic(expected = "agent_name cannot be empty")]
    fn empty_agent_name_panics() {
        let mem = HierarchicalMemoryLocal::new("x");
        GraphRagRetriever::new(&mem, "");
    }
}
