//! Multi-agent hive mind orchestration with 4-layer architecture.
//!
//! Provides a shared knowledge graph, event bus, gossip protocol,
//! CRDT-based convergence, a controller/orchestrator for managing
//! swarms, hive events, learning feed, and evaluation.

pub mod bloom;
pub mod controller;
pub mod crdt;
pub mod dht;
pub mod embeddings;
pub mod error;
pub mod event_bus;
pub mod fact_lifecycle;
pub mod feed;
pub mod gossip;
pub mod graph;
pub mod hive_eval;
pub mod hive_events;
pub mod models;
pub mod orchestrator;
pub mod quality;
pub mod query_expansion;
pub mod reranker;
pub mod workload;

pub use bloom::BloomFilter;
pub use controller::HiveController;
pub use crdt::{GCounter, GSet, LWWRegister, ORSet, PNCounter};
pub use dht::{DHTRouter, HashRing, ShardFact, ShardStore};
pub use embeddings::{cosine_similarity, cosine_similarity_batch, dot_product, normalize};
pub use error::{HiveError, Result};
pub use event_bus::{EventBus, LocalEventBus};
pub use fact_lifecycle::{decay_confidence, gc_expired_facts, refresh_confidence, FactTTL};
pub use feed::{FeedConfig, FeedResult, run_feed};
pub use gossip::{GossipProtocol, convergence_check};
pub use graph::search::{ScoredFact as KeywordScoredFact, tokenize, word_overlap};
pub use graph::HiveGraph;
pub use graph::{
    BROADCAST_TAG_PREFIX, CONFIDENCE_SCORE_BOOST, ESCALATION_TAG_PREFIX,
    GOSSIP_TAG_PREFIX as GRAPH_GOSSIP_TAG_PREFIX,
};
pub use hive_eval::{HiveEvalConfig, HiveEvalResult, QueryResult, run_eval};
pub use hive_events::{
    ALL_HIVE_TOPICS, HIVE_AGENT_READY, HIVE_FEED_COMPLETE, HIVE_LEARN_CONTENT, HIVE_QUERY,
    HIVE_QUERY_RESPONSE,
};
pub use models::{
    AgentSpec, BusEvent, GossipConfig, GossipMessage, GraphStats, HiveAgent,
    HiveEdge, HiveFact, HiveManifest, HiveState, MergeResult,
};
pub use orchestrator::{DefaultPromotionPolicy, HiveMindOrchestrator, PromotionPolicy};
pub use quality::{score_content_quality, QualityGate};
pub use query_expansion::{expand_query, search_expanded};
pub use reranker::{
    hybrid_score, hybrid_score_weighted, rrf_merge, rrf_merge_scored, trust_weighted_score,
    ScoredFact,
};
pub use workload::{HiveEvent, HiveWorkloadConfig, WorkloadStatus};
