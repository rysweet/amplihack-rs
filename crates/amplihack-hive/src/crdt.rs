use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A grow-only counter CRDT.
///
/// Each node maintains its own monotonically increasing count; the
/// global value is the sum of all per-node counts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GCounter {
    counts: HashMap<String, u64>,
}

impl GCounter {
    /// Create an empty counter.
    pub fn new() -> Self {
        Self {
            counts: HashMap::new(),
        }
    }

    /// Increment the counter for `node_id` and return the new local count.
    pub fn increment(&mut self, _node_id: &str) -> u64 {
        todo!()
    }

    /// Return the total value across all nodes.
    pub fn value(&self) -> u64 {
        todo!()
    }

    /// Merge another counter into this one (element-wise max).
    pub fn merge(&mut self, _other: &GCounter) -> &mut Self {
        todo!()
    }

    /// Return the count for a single node.
    pub fn get(&self, _node_id: &str) -> u64 {
        todo!()
    }
}

impl Default for GCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// A last-writer-wins register CRDT.
///
/// Concurrency is resolved by timestamp; ties are broken by node ID.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LWWRegister<T: Clone> {
    value: Option<T>,
    timestamp: u64,
    node_id: String,
}

impl<T: Clone> LWWRegister<T> {
    /// Create an empty register owned by `node_id`.
    pub fn new(node_id: String) -> Self {
        Self {
            value: None,
            timestamp: 0,
            node_id,
        }
    }

    /// Set the register value at the given timestamp.
    pub fn set(&mut self, _value: T, _timestamp: u64) {
        todo!()
    }

    /// Get the current value, if any.
    pub fn get(&self) -> Option<&T> {
        todo!()
    }

    /// Merge another register into this one.
    pub fn merge(&mut self, _other: &LWWRegister<T>) {
        todo!()
    }

    /// Return the timestamp of the last write.
    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// Return the owning node ID.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }
}
