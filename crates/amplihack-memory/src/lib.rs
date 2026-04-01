//! amplihack-memory: Five-type cognitive memory system.
//!
//! Provides memory coordination, distributed sharding, bloom filter dedup,
//! and session discovery — matching the Python amplihack memory subsystem.

pub mod bloom;
pub mod config;
pub mod coordinator;
pub mod discoveries;
pub mod distributed_store;
pub mod graph_store;
pub mod hash_ring;
pub mod memory_store;
pub mod models;
pub mod quality;

pub use bloom::BloomFilter;
pub use config::{Backend, MemoryConfig, Topology, Transport};
pub use coordinator::MemoryCoordinator;
pub use discoveries::{Discovery, get_recent_discoveries, store_discovery};
pub use distributed_store::DistributedGraphStore;
pub use graph_store::GraphStore;
pub use hash_ring::HashRing;
pub use memory_store::InMemoryGraphStore;
pub use models::{MemoryEntry, MemoryQuery, MemoryType, SessionInfo, StorageRequest};
