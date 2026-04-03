//! Distributed hive mind — event-driven multi-agent coordination.
//!
//! Ported from the Python `hive_mind.distributed` package.

mod coordinator;
mod graph;
mod hive_mind;
pub mod memory;
pub mod merge;
mod node;
mod transport;

pub use coordinator::HiveCoordinator;
pub use graph::DistributedHiveGraph;
pub use hive_mind::DistributedHiveMind;
pub use memory::DistributedCognitiveMemory;
pub use node::AgentNode;
pub use transport::{EventBusShardTransport, LocalShardTransport, ShardTransport};

/// Maximum number of incorporated event IDs tracked per agent (FIFO eviction).
pub const MAX_INCORPORATED_EVENTS: usize = 100_000;

/// Maximum number of contradictions tracked by the coordinator.
pub const MAX_CONTRADICTIONS: usize = 10_000;

/// Default query fan-out for shard queries.
pub const DEFAULT_QUERY_MAX_WORKERS: usize = 8;

/// Position-score decrement per rank (RRF-style merge).
pub const POSITION_SCORE_DECREMENT: f64 = 0.05;

/// Weight for unigram overlap in relevance scoring.
pub const UNIGRAM_WEIGHT: f64 = 0.7;

/// Weight for bigram overlap in relevance scoring.
pub const BIGRAM_WEIGHT: f64 = 0.3;

/// Multiplier for hive search limit relative to requested limit.
pub const HIVE_SEARCH_MULTIPLIER: usize = 3;

/// Maximum keywords extracted from a query for concept search.
pub const QUERY_KEYWORD_LIMIT: usize = 10;
