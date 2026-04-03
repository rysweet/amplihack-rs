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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_saturates_instead_of_overflowing() {
        let mut counter = GCounter::new();
        counter.counts.insert("a".into(), u64::MAX);
        counter.counts.insert("b".into(), 1);
        assert_eq!(counter.value(), u64::MAX);
    }

    #[test]
    fn increment_saturates() {
        let mut counter = GCounter::new();
        counter.counts.insert("n".into(), u64::MAX);
        let val = counter.increment("n");
        assert_eq!(val, u64::MAX);
    }

    #[test]
    fn merge_takes_element_wise_max() {
        let mut a = GCounter::new();
        a.increment("x");
        a.increment("x");
        let mut b = GCounter::new();
        b.increment("x");
        b.increment("y");
        a.merge(&b);
        assert_eq!(a.get("x"), 2);
        assert_eq!(a.get("y"), 1);
    }

    #[test]
    fn default_is_zero() {
        let c = GCounter::default();
        assert_eq!(c.value(), 0);
    }

    #[test]
    fn merge_is_idempotent() {
        let mut a = GCounter::new();
        a.increment("x");
        let b = a.clone();
        a.merge(&b);
        assert_eq!(a.value(), 1);
    }
}
