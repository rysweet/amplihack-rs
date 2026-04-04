//! Graph database adapters and query definitions.
//!
//! Provides the [`DbManager`] trait for graph database operations and
//! Cypher query constants used by concrete implementations.

pub mod manager;
pub mod queries;
pub mod types;

pub use manager::{DbEnvironment, DbManager, QueryParams};
pub use types::*;
