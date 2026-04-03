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
        self.counts
            .values()
            .fold(0u64, |acc, &v| acc.saturating_add(v))
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

    /// Set the register value at the given timestamp, restoring the owner node ID.
    pub fn set(&mut self, value: T, timestamp: u64, node_id: &str) {
        if timestamp > self.timestamp {
            self.value = Some(value);
            self.timestamp = timestamp;
            self.node_id = node_id.to_string();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcounter_value_saturates_instead_of_overflowing() {
        let mut counter = GCounter::new();
        counter.counts.insert("a".into(), u64::MAX);
        counter.counts.insert("b".into(), 1);
        assert_eq!(counter.value(), u64::MAX);
    }

    #[test]
    fn gcounter_increment_saturates() {
        let mut counter = GCounter::new();
        counter.counts.insert("n".into(), u64::MAX);
        let val = counter.increment("n");
        assert_eq!(val, u64::MAX);
    }

    #[test]
    fn lww_register_tie_breaks_by_node_id() {
        let mut r1 = LWWRegister::new("node-a".into());
        r1.set("value-a", 10, "node-a");
        let mut r2 = LWWRegister::new("node-b".into());
        r2.set("value-b", 10, "node-b");
        // node-b > node-a, so merging r2 into r1 should adopt r2's value
        r1.merge(&r2);
        assert_eq!(r1.get(), Some(&"value-b"));
        assert_eq!(r1.node_id(), "node-b");
    }

    #[test]
    fn lww_register_set_restores_node_id_after_merge() {
        let mut r1 = LWWRegister::new("node-a".into());
        r1.set("v1", 5, "node-a");
        let mut r2 = LWWRegister::new("node-b".into());
        r2.set("v2", 10, "node-b");
        // merge overwrites r1's node_id to "node-b"
        r1.merge(&r2);
        assert_eq!(r1.node_id(), "node-b");
        // set must restore node_id to "node-a"
        r1.set("v3", 15, "node-a");
        assert_eq!(r1.node_id(), "node-a");
        assert_eq!(r1.get(), Some(&"v3"));
    }
}
