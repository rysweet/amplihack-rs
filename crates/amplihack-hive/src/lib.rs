//! Multi-agent hive mind orchestration with 4-layer architecture.
//!
//! Provides a shared knowledge graph, event bus, gossip protocol,
//! CRDT-based convergence, a controller/orchestrator for managing
//! swarms, hive events, learning feed, and evaluation.

pub mod controller;
pub mod crdt;
pub mod error;
pub mod event_bus;
pub mod feed;
pub mod gossip;
pub mod graph;
pub mod hive_eval;
pub mod hive_events;
pub mod models;
pub mod orchestrator;
pub mod workload;

pub use controller::HiveController;
pub use crdt::{GCounter, LWWRegister};
pub use error::{HiveError, Result};
pub use event_bus::{EventBus, LocalEventBus};
pub use feed::{FeedConfig, FeedResult, run_feed};
pub use gossip::GossipProtocol;
pub use graph::HiveGraph;
pub use hive_eval::{HiveEvalConfig, HiveEvalResult, QueryResult, run_eval};
pub use hive_events::{
    ALL_HIVE_TOPICS, HIVE_AGENT_READY, HIVE_FEED_COMPLETE, HIVE_LEARN_CONTENT, HIVE_QUERY,
    HIVE_QUERY_RESPONSE,
};
pub use models::{
    AgentSpec, BusEvent, GossipConfig, GossipMessage, HiveFact, HiveManifest,
    HiveState, MergeResult,
};
pub use orchestrator::{DefaultPromotionPolicy, HiveMindOrchestrator, PromotionPolicy};
pub use workload::{HiveEvent, HiveWorkloadConfig, WorkloadStatus};
