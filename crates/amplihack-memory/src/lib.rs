//! amplihack-memory: Five-type cognitive memory system.
//!
//! Provides memory coordination, distributed sharding, bloom filter dedup,
//! and session discovery — matching the Python amplihack memory subsystem.

pub mod auto_backend;
/// Lazy memory library availability check.
pub mod auto_install;
pub mod backend;
pub mod bloom;
pub mod config;
pub mod coordinator;
pub mod database;
pub(crate) mod database_helpers;
pub mod discoveries;
pub mod distributed_store;
pub mod evaluation;
pub mod facade;
pub mod graph_db;
pub mod graph_store;
pub mod hash_ring;
pub mod maintenance;
pub mod manager;
pub mod memory_store;
pub mod models;
pub mod network_store;
pub(crate) mod network_store_types;
pub mod quality;
pub mod retrieval;
pub mod retrieval_pipeline;
pub mod sqlite_backend;
pub mod storage_pipeline;

#[cfg(feature = "pyo3-bindings")]
pub mod pyo3_bindings;

pub use auto_backend::{DetectedBackend, detect_backend};
pub use backend::{BackendHealth, InMemoryBackend, MemoryBackend};
pub use bloom::BloomFilter;
pub use config::{Backend, MemoryConfig, Topology, Transport};
pub use coordinator::MemoryCoordinator;
#[cfg(feature = "sqlite")]
pub use database::MemoryDatabase;
pub use discoveries::{Discovery, get_recent_discoveries, store_discovery};
pub use distributed_store::DistributedGraphStore;
pub use evaluation::{
    BackendComparison, BackendReliabilityEvaluator, BackendReliabilityMetrics, BenchmarkEvaluator,
    BenchmarkMetrics, ComparisonReport, PerformanceContracts, QualityEvaluator, QualityMetrics,
    QualityReport, QueryTestCase, RetrievalQualityEvaluator, RetrievalQualityMetrics,
};
pub use facade::MemoryFacade;
pub use graph_store::GraphStore;
pub use hash_ring::HashRing;
#[cfg(feature = "sqlite")]
pub use maintenance::MemoryMaintenance;
pub use manager::MemoryManager;
pub use memory_store::InMemoryGraphStore;
pub use models::{MemoryEntry, MemoryQuery, MemoryType, SessionInfo, StorageRequest};
pub use network_store::{AgentRegistry, NetworkGraphStore};
pub use retrieval::{Fact, IntentKind, MemorySearch as RetrievalMemorySearch};
pub use retrieval_pipeline::{RetrievalPipeline, RetrievalResult, ScoredEntry};
pub use storage_pipeline::{StoragePipeline, StorageResult};
