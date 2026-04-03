use serde::{Deserialize, Serialize};

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
    fn tie_breaks_by_node_id() {
        let mut r1 = LWWRegister::new("node-a".into());
        r1.set("value-a", 10, "node-a");
        let mut r2 = LWWRegister::new("node-b".into());
        r2.set("value-b", 10, "node-b");
        r1.merge(&r2);
        assert_eq!(r1.get(), Some(&"value-b"));
        assert_eq!(r1.node_id(), "node-b");
    }

    #[test]
    fn set_restores_node_id_after_merge() {
        let mut r1 = LWWRegister::new("node-a".into());
        r1.set("v1", 5, "node-a");
        let mut r2 = LWWRegister::new("node-b".into());
        r2.set("v2", 10, "node-b");
        r1.merge(&r2);
        assert_eq!(r1.node_id(), "node-b");
        r1.set("v3", 15, "node-a");
        assert_eq!(r1.node_id(), "node-a");
        assert_eq!(r1.get(), Some(&"v3"));
    }

    #[test]
    fn earlier_timestamp_ignored() {
        let mut r = LWWRegister::new("a".into());
        r.set("first", 10, "a");
        r.set("second", 5, "a");
        assert_eq!(r.get(), Some(&"first"));
    }

    #[test]
    fn merge_empty_into_populated() {
        let mut r1 = LWWRegister::new("a".into());
        r1.set("hello", 5, "a");
        let r2: LWWRegister<&str> = LWWRegister::new("b".into());
        r1.merge(&r2);
        assert_eq!(r1.get(), Some(&"hello"));
    }

    #[test]
    fn merge_populated_into_empty() {
        let mut r1: LWWRegister<&str> = LWWRegister::new("a".into());
        let mut r2 = LWWRegister::new("b".into());
        r2.set("hello", 5, "b");
        r1.merge(&r2);
        assert_eq!(r1.get(), Some(&"hello"));
    }
}
