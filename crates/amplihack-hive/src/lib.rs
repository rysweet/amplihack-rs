//! Multi-agent hive mind orchestration with 4-layer architecture.
//!
//! Provides a shared knowledge graph, event bus, gossip protocol,
//! CRDT-based convergence, and a controller/orchestrator for
//! managing swarms of cooperating agents.

pub mod controller;
pub mod crdt;
pub mod error;
pub mod event_bus;
pub mod gossip;
pub mod graph;
pub mod models;
pub mod orchestrator;
pub mod workload;

pub use controller::HiveController;
pub use crdt::{GCounter, LWWRegister};
pub use error::{HiveError, Result};
pub use event_bus::{EventBus, LocalEventBus};
pub use gossip::GossipProtocol;
pub use graph::HiveGraph;
pub use models::{
    AgentSpec, BusEvent, GossipConfig, GossipMessage, HiveFact, HiveManifest,
    HiveState, MergeResult,
};
pub use orchestrator::{DefaultPromotionPolicy, HiveMindOrchestrator, PromotionPolicy};
pub use workload::{HiveEvent, HiveWorkloadConfig, WorkloadStatus};
