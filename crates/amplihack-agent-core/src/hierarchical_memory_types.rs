//! Core types for the hierarchical memory system.
//!
//! Port of Python `_hierarchical_memory_local.py` type definitions:
//! - [`MemoryCategory`] — five cognitive memory categories
//! - [`KnowledgeNode`] — graph node
//! - [`KnowledgeEdge`] — graph edge
//! - [`KnowledgeSubgraph`] — subgraph result container with LLM formatting
//! - [`MemoryClassifier`] — rule-based category classifier

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── MemoryCategory ───────────────────────────────────────────────────────

/// Categories of memory matching cognitive science model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    Episodic,
    Semantic,
    Procedural,
    Prospective,
    Working,
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Episodic => write!(f, "episodic"),
            Self::Semantic => write!(f, "semantic"),
            Self::Procedural => write!(f, "procedural"),
            Self::Prospective => write!(f, "prospective"),
            Self::Working => write!(f, "working"),
        }
    }
}

// ── KnowledgeNode ────────────────────────────────────────────────────────

/// A node in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeNode {
    pub node_id: String,
    pub category: MemoryCategory,
    pub content: String,
    pub concept: String,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default)]
    pub source_id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

fn default_confidence() -> f64 { 0.8 }

// ── KnowledgeEdge ────────────────────────────────────────────────────────

/// An edge in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEdge {
    pub source_id: String,
    pub target_id: String,
    pub relationship: String,
    #[serde(default = "default_weight")]
    pub weight: f64,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

fn default_weight() -> f64 { 1.0 }

// ── KnowledgeSubgraph ────────────────────────────────────────────────────

/// A subgraph of knowledge nodes and edges returned by retrieval.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowledgeSubgraph {
    pub nodes: Vec<KnowledgeNode>,
    pub edges: Vec<KnowledgeEdge>,
    #[serde(default)]
    pub query: String,
}

impl KnowledgeSubgraph {
    pub fn new(query: impl Into<String>) -> Self {
        Self { nodes: Vec::new(), edges: Vec::new(), query: query.into() }
    }

    /// Format subgraph as LLM-readable context string.
    pub fn to_llm_context(&self, chronological: bool) -> String {
        if self.nodes.is_empty() { return String::new(); }
        let mut nodes = self.nodes.clone();
        if chronological {
            nodes.sort_by_key(|n| {
                n.metadata.get("temporal_index").and_then(|v| v.as_i64()).unwrap_or(0)
            });
        } else {
            nodes.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal));
        }
        let mut out = String::new();
        for (i, n) in nodes.iter().enumerate() {
            out.push_str(&format!("{}. [{}] {}: {} (confidence: {:.2})\n",
                i + 1, n.category, n.concept, n.content, n.confidence));
        }
        if !self.edges.is_empty() {
            out.push_str("\nRelationships:\n");
            for e in &self.edges {
                out.push_str(&format!("  {} --[{} {:.2}]--> {}\n",
                    e.source_id, e.relationship, e.weight, e.target_id));
            }
        }
        out
    }
}

// ── MemoryClassifier ─────────────────────────────────────────────────────

/// Rule-based classifier mapping content to a [`MemoryCategory`].
pub struct MemoryClassifier;

impl MemoryClassifier {
    /// Classify content into a memory category using keyword heuristics.
    pub fn classify(content: &str) -> MemoryCategory {
        let lower = content.to_lowercase();
        if Self::is_procedural(&lower) { return MemoryCategory::Procedural; }
        if Self::is_prospective(&lower) { return MemoryCategory::Prospective; }
        if Self::is_episodic(&lower) { return MemoryCategory::Episodic; }
        MemoryCategory::Semantic
    }

    fn is_procedural(text: &str) -> bool {
        ["step 1", "step 2", "how to", "procedure", "instructions", "recipe",
         "guide", "tutorial", "method", "process:", "1.", "2.", "first,", "then,", "finally,"]
            .iter().any(|m| text.contains(m))
    }

    fn is_prospective(text: &str) -> bool {
        ["plan to", "will need", "should do", "reminder", "todo", "next step",
         "goal:", "objective:", "upcoming", "schedule"]
            .iter().any(|m| text.contains(m))
    }

    fn is_episodic(text: &str) -> bool {
        ["happened", "occurred", "event:", "incident", "observed",
         "witnessed", "experience:", "session:", "meeting:"]
            .iter().any(|m| text.contains(m))
    }
}

// ── StoreKnowledgeParams ─────────────────────────────────────────────────

/// Parameters for [`super::hierarchical_memory_local::HierarchicalMemoryLocal::store_knowledge`].
pub struct StoreKnowledgeParams<'a> {
    pub content: &'a str,
    pub concept: &'a str,
    pub confidence: f64,
    pub category: MemoryCategory,
    pub source_id: &'a str,
    pub tags: &'a [String],
    pub temporal_metadata: Option<&'a HashMap<String, serde_json::Value>>,
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_category_display_and_serde() {
        assert_eq!(MemoryCategory::Episodic.to_string(), "episodic");
        assert_eq!(MemoryCategory::Semantic.to_string(), "semantic");
        assert_eq!(MemoryCategory::Procedural.to_string(), "procedural");
        assert_eq!(MemoryCategory::Prospective.to_string(), "prospective");
        assert_eq!(MemoryCategory::Working.to_string(), "working");
        let json = serde_json::to_string(&MemoryCategory::Semantic).unwrap();
        let parsed: MemoryCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, MemoryCategory::Semantic);
    }

    #[test]
    fn classifier_categories() {
        assert_eq!(MemoryClassifier::classify("Paris is the capital"), MemoryCategory::Semantic);
        assert_eq!(MemoryClassifier::classify("Step 1: preheat oven"), MemoryCategory::Procedural);
        assert_eq!(MemoryClassifier::classify("How to bake a cake"), MemoryCategory::Procedural);
        assert_eq!(MemoryClassifier::classify("Plan to review tomorrow"), MemoryCategory::Prospective);
        assert_eq!(MemoryClassifier::classify("Reminder: call dentist"), MemoryCategory::Prospective);
        assert_eq!(MemoryClassifier::classify("An incident occurred"), MemoryCategory::Episodic);
    }

    #[test]
    fn subgraph_to_llm_context() {
        let sg = KnowledgeSubgraph {
            nodes: vec![KnowledgeNode {
                node_id: "n1".into(), category: MemoryCategory::Semantic,
                content: "Cells are alive".into(), concept: "Biology".into(),
                confidence: 0.9, source_id: String::new(), created_at: String::new(),
                tags: vec![], metadata: HashMap::new(),
            }],
            edges: vec![], query: "cells".into(),
        };
        let ctx = sg.to_llm_context(false);
        assert!(ctx.contains("Biology") && ctx.contains("0.90"));
    }

    #[test]
    fn knowledge_node_serde_roundtrip() {
        let node = KnowledgeNode {
            node_id: "n1".into(), category: MemoryCategory::Semantic,
            content: "test".into(), concept: "topic".into(), confidence: 0.9,
            source_id: String::new(), created_at: String::new(),
            tags: vec!["t1".into()], metadata: HashMap::new(),
        };
        let json = serde_json::to_string(&node).unwrap();
        let parsed: KnowledgeNode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.node_id, "n1");
        assert_eq!(parsed.category, MemoryCategory::Semantic);
    }

    #[test]
    fn knowledge_edge_serde_defaults() {
        let json = r#"{"source_id":"a","target_id":"b","relationship":"SIMILAR_TO"}"#;
        let edge: KnowledgeEdge = serde_json::from_str(json).unwrap();
        assert!((edge.weight - 1.0).abs() < f64::EPSILON);
    }
}
