use serde::{Deserialize, Serialize};

use super::gcounter::GCounter;

/// A positive-negative counter CRDT.
///
/// Composed of two [`GCounter`]s: one for increments (P) and one for
/// decrements (N).  The value is `P − N`.  This allows both increment
/// and decrement operations while maintaining CRDT merge properties.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PNCounter {
    positive: GCounter,
    negative: GCounter,
}

impl PNCounter {
    /// Create a counter starting at zero.
    pub fn new() -> Self {
        Self {
            positive: GCounter::new(),
            negative: GCounter::new(),
        }
    }

    /// Increment the counter for `node_id`.
    pub fn increment(&mut self, node_id: &str) {
        self.positive.increment(node_id);
    }

    /// Decrement the counter for `node_id`.
    pub fn decrement(&mut self, node_id: &str) {
        self.negative.increment(node_id);
    }

    /// Return the net value (positive − negative) as a signed integer.
    pub fn value(&self) -> i128 {
        i128::from(self.positive.value()) - i128::from(self.negative.value())
    }

    /// Merge another counter into this one.
    pub fn merge(&mut self, other: &PNCounter) -> &mut Self {
        self.positive.merge(&other.positive);
        self.negative.merge(&other.negative);
        self
    }
}

impl Default for PNCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increment_and_decrement() {
        let mut c = PNCounter::new();
        c.increment("a");
        c.increment("a");
        c.decrement("a");
        assert_eq!(c.value(), 1);
    }

    #[test]
    fn merge_combines_both_halves() {
        let mut a = PNCounter::new();
        a.increment("x");
        a.increment("x");
        let mut b = PNCounter::new();
        b.decrement("y");
        a.merge(&b);
        assert_eq!(a.value(), 1); // 2 - 1
    }

    #[test]
    fn value_can_be_negative() {
        let mut c = PNCounter::new();
        c.decrement("a");
        c.decrement("a");
        c.increment("a");
        assert_eq!(c.value(), -1);
    }

    #[test]
    fn default_is_zero() {
        let c = PNCounter::default();
        assert_eq!(c.value(), 0);
    }

    #[test]
    fn merge_is_idempotent() {
        let mut a = PNCounter::new();
        a.increment("x");
        a.decrement("y");
        let b = a.clone();
        a.merge(&b);
        assert_eq!(a.value(), 0); // 1 - 1
    }

    #[test]
    fn merge_is_commutative() {
        let mut a = PNCounter::new();
        a.increment("x");
        let mut b = PNCounter::new();
        b.decrement("y");

        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab.value(), ba.value());
    }
}
