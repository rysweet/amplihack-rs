//! Type definitions for the Knowledge Builder.
//!
//! Ported from `amplihack/knowledge_builder/kb_types.py`.
//!
//! These lightweight structs model a Socratic-style knowledge graph that the
//! Knowledge Builder populates through iterative question generation and web
//! search.

use serde::{Deserialize, Serialize};

/// A question in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Question {
    /// The question text.
    pub text: String,
    /// Depth level: 0 = initial, 1–3 = Socratic follow-ups.
    pub depth: u32,
    /// Index of the parent question, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index: Option<usize>,
    /// Populated after web search.
    #[serde(default)]
    pub answer: String,
}

/// An answer with source attribution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Answer {
    /// The answer text.
    pub text: String,
    /// URLs or references that sourced this answer.
    #[serde(default)]
    pub sources: Vec<String>,
}

/// A knowledge triplet (subject → predicate → object).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeTriplet {
    /// Subject entity.
    pub subject: String,
    /// Relationship / predicate.
    pub predicate: String,
    /// Object entity.
    pub object: String,
    /// Source attribution for this triplet.
    pub source: String,
}

/// Complete knowledge graph for a topic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeGraph {
    /// The root topic.
    pub topic: String,
    /// Questions explored during research.
    #[serde(default)]
    pub questions: Vec<Question>,
    /// Extracted knowledge triplets.
    #[serde(default)]
    pub triplets: Vec<KnowledgeTriplet>,
    /// Aggregated source URLs.
    #[serde(default)]
    pub sources: Vec<String>,
    /// ISO-8601 timestamp of creation.
    #[serde(default)]
    pub timestamp: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_serialization_roundtrip() {
        let q = Question {
            text: "What is Rust?".into(),
            depth: 0,
            parent_index: None,
            answer: String::new(),
        };
        let json = serde_json::to_string(&q).unwrap();
        let q2: Question = serde_json::from_str(&json).unwrap();
        assert_eq!(q, q2);
    }

    #[test]
    fn answer_with_sources() {
        let a = Answer {
            text: "A systems language".into(),
            sources: vec!["https://rust-lang.org".into()],
        };
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("rust-lang.org"));
        let a2: Answer = serde_json::from_str(&json).unwrap();
        assert_eq!(a, a2);
    }

    #[test]
    fn knowledge_triplet_fields() {
        let t = KnowledgeTriplet {
            subject: "Rust".into(),
            predicate: "is_a".into(),
            object: "programming language".into(),
            source: "docs".into(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let t2: KnowledgeTriplet = serde_json::from_str(&json).unwrap();
        assert_eq!(t, t2);
    }

    #[test]
    fn knowledge_graph_defaults() {
        let json = r#"{"topic":"Rust"}"#;
        let g: KnowledgeGraph = serde_json::from_str(json).unwrap();
        assert_eq!(g.topic, "Rust");
        assert!(g.questions.is_empty());
        assert!(g.triplets.is_empty());
        assert!(g.sources.is_empty());
        assert!(g.timestamp.is_empty());
    }

    #[test]
    fn knowledge_graph_full_roundtrip() {
        let g = KnowledgeGraph {
            topic: "AI".into(),
            questions: vec![Question {
                text: "What is AI?".into(),
                depth: 0,
                parent_index: None,
                answer: "Artificial intelligence".into(),
            }],
            triplets: vec![KnowledgeTriplet {
                subject: "AI".into(),
                predicate: "includes".into(),
                object: "ML".into(),
                source: "wiki".into(),
            }],
            sources: vec!["https://example.com".into()],
            timestamp: "2024-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string_pretty(&g).unwrap();
        let g2: KnowledgeGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(g, g2);
    }

    #[test]
    fn parent_index_skipped_when_none() {
        let q = Question {
            text: "q".into(),
            depth: 0,
            parent_index: None,
            answer: String::new(),
        };
        let json = serde_json::to_string(&q).unwrap();
        assert!(!json.contains("parent_index"));
    }
}
