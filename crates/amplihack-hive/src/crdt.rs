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
    pub fn increment(&mut self, node_id: &str) -> u64 {
        let count = self.counts.entry(node_id.to_string()).or_insert(0);
        *count = count.saturating_add(1);
        *count
    }

    /// Return the total value across all nodes.
    pub fn value(&self) -> u64 {
        self.counts.values().sum()
    }

    /// Merge another counter into this one (element-wise max).
    pub fn merge(&mut self, other: &GCounter) -> &mut Self {
        for (node, &count) in &other.counts {
            let entry = self.counts.entry(node.clone()).or_insert(0);
            *entry = (*entry).max(count);
        }
        self
    }

    /// Return the count for a single node.
    pub fn get(&self, node_id: &str) -> u64 {
        self.counts.get(node_id).copied().unwrap_or(0)
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
    pub fn set(&mut self, value: T, timestamp: u64) {
        if timestamp > self.timestamp {
            self.value = Some(value);
            self.timestamp = timestamp;
        }
    }

    /// Get the current value, if any.
    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }

    /// Merge another register into this one.
    /// When timestamps are equal, the higher node ID wins (deterministic tie-break).
    pub fn merge(&mut self, other: &LWWRegister<T>) {
        if other.timestamp > self.timestamp
            || (other.timestamp == self.timestamp && other.node_id > self.node_id)
        {
            self.value = other.value.clone();
            self.timestamp = other.timestamp;
            self.node_id = other.node_id.clone();
        }
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
