use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A grow-only set CRDT.
///
/// Items can be added but never removed.  Merge is the set union,
/// which is commutative, associative, and idempotent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GSet {
    items: BTreeSet<String>,
}

impl GSet {
    /// Create an empty set.
    pub fn new() -> Self {
        Self {
            items: BTreeSet::new(),
        }
    }

    /// Add an item to the set.
    pub fn add(&mut self, item: impl Into<String>) {
        self.items.insert(item.into());
    }

    /// Check whether `item` is in the set.
    pub fn contains(&self, item: &str) -> bool {
        self.items.contains(item)
    }

    /// Merge another set into this one (set union).
    pub fn merge(&mut self, other: &GSet) -> &mut Self {
        for item in &other.items {
            self.items.insert(item.clone());
        }
        self
    }

    /// Return a reference to the underlying items.
    pub fn items(&self) -> &BTreeSet<String> {
        &self.items
    }

    /// Return the number of items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Return `true` if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl Default for GSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_contains() {
        let mut s = GSet::new();
        assert!(!s.contains("a"));
        s.add("a");
        assert!(s.contains("a"));
    }

    #[test]
    fn merge_is_union() {
        let mut a = GSet::new();
        a.add("x");
        a.add("y");
        let mut b = GSet::new();
        b.add("y");
        b.add("z");
        a.merge(&b);
        assert!(a.contains("x"));
        assert!(a.contains("y"));
        assert!(a.contains("z"));
        assert_eq!(a.len(), 3);
    }

    #[test]
    fn merge_is_idempotent() {
        let mut a = GSet::new();
        a.add("x");
        let b = a.clone();
        a.merge(&b);
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn merge_is_commutative() {
        let mut a = GSet::new();
        a.add("x");
        let mut b = GSet::new();
        b.add("y");

        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab, ba);
    }

    #[test]
    fn default_is_empty() {
        let s = GSet::default();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn items_returns_sorted_view() {
        let mut s = GSet::new();
        s.add("c");
        s.add("a");
        s.add("b");
        let items: Vec<&String> = s.items().iter().collect();
        assert_eq!(items, vec!["a", "b", "c"]);
    }
}
