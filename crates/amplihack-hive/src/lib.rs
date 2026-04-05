//! Multi-agent hive mind orchestration with 4-layer architecture.

pub mod bloom;
pub mod controller;
pub mod crdt;
pub mod dht;
pub mod distributed;
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
pub use controller::{HiveController, InMemoryGateway, InMemoryGraphStore};
pub use crdt::{GCounter, GSet, LWWRegister, ORSet, PNCounter};
pub use dht::{DHTRouter, HashRing, ShardFact, ShardStore};
pub use embeddings::{cosine_similarity, cosine_similarity_batch, dot_product, normalize};
pub use error::{HiveError, Result};
pub use event_bus::{EventBus, LocalEventBus, MAX_MAILBOX_SIZE};
pub use fact_lifecycle::{FactTTL, decay_confidence, gc_expired_facts, refresh_confidence};
pub use feed::{FeedConfig, FeedResult, run_feed};
pub use gossip::{GossipProtocol, convergence_check};
pub use graph::HiveGraph;
pub use graph::search::{ScoredFact as KeywordScoredFact, tokenize, word_overlap};
pub use graph::{
    BROADCAST_TAG_PREFIX, CONFIDENCE_SCORE_BOOST, ESCALATION_TAG_PREFIX,
    GOSSIP_TAG_PREFIX as GRAPH_GOSSIP_TAG_PREFIX,
};
pub use hive_eval::{
    EvalResponder, HiveEvalConfig, HiveEvalResult, QueryResult, run_eval, run_eval_with_responder,
};
pub use hive_events::{
    ALL_HIVE_TOPICS, HIVE_AGENT_READY, HIVE_FEED_COMPLETE, HIVE_LEARN_CONTENT, HIVE_QUERY,
    HIVE_QUERY_RESPONSE,
};
pub use models::{
    AgentSpec, BusEvent, DEFAULT_BROADCAST_THRESHOLD, DEFAULT_CONFIDENCE_GATE,
    DEFAULT_CONTRADICTION_OVERLAP, DEFAULT_QUALITY_THRESHOLD, DEFAULT_TRUST_SCORE, EventBusConfig,
    FACT_ID_HEX_LENGTH, GOSSIP_MIN_CONFIDENCE, GatewayConfig, GossipConfig, GossipMessage,
    GraphStats, GraphStoreConfig, HiveAgent, HiveEdge, HiveFact, HiveManifest, HiveState,
    MAX_TRUST_SCORE, MergeResult, PEER_CONFIDENCE_DISCOUNT, make_event,
};
pub use orchestrator::{
    DefaultPromotionPolicy, HiveMindOrchestrator, PromotionPolicy, PromotionResult,
};
pub use quality::{QualityGate, score_content_quality};
pub use query_expansion::{expand_query, search_expanded};
pub use reranker::{
    ScoredFact, hybrid_score, hybrid_score_weighted, rrf_merge, rrf_merge_scored,
    trust_weighted_score,
};
pub use workload::{HiveEvent, HiveWorkloadConfig, WorkloadStatus};
