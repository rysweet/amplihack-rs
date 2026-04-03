use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An Observed-Remove set CRDT.
///
/// Each `add()` assigns a unique tag to the element.  `remove()` records all
/// tags currently associated with the element in a tombstone set.  An element
/// is present iff it has at least one tag NOT in the tombstone set.
///
/// Merge unions both the element-tag pairs and the tombstones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ORSet {
    elements: BTreeMap<String, BTreeSet<String>>,
    tombstones: BTreeMap<String, BTreeSet<String>>,
}

impl ORSet {
    /// Create an empty set.
    pub fn new() -> Self {
        Self {
            elements: BTreeMap::new(),
            tombstones: BTreeMap::new(),
        }
    }

    /// Add `item` with a fresh unique tag.  Returns the tag.
    pub fn add(&mut self, item: impl Into<String>) -> String {
        let tag = Uuid::new_v4().to_string();
        self.elements
            .entry(item.into())
            .or_default()
            .insert(tag.clone());
        tag
    }

    /// Remove `item` by tombstoning all its currently-visible tags.
    pub fn remove(&mut self, item: &str) {
        if let Some(tags) = self.elements.get(item) {
            let tombstone = self.tombstones.entry(item.to_string()).or_default();
            tombstone.extend(tags.iter().cloned());
        }
    }

    /// Check whether `item` is in the set (has at least one live tag).
    pub fn contains(&self, item: &str) -> bool {
        let tags = match self.elements.get(item) {
            Some(t) => t,
            None => return false,
        };
        match self.tombstones.get(item) {
            Some(dead) => tags.iter().any(|t| !dead.contains(t)),
            None => !tags.is_empty(),
        }
    }

    /// Merge another set into this one (union of elements and tombstones).
    pub fn merge(&mut self, other: &ORSet) -> &mut Self {
        for (item, tags) in &other.elements {
            self.elements
                .entry(item.clone())
                .or_default()
                .extend(tags.iter().cloned());
        }
        for (item, tags) in &other.tombstones {
            self.tombstones
                .entry(item.clone())
                .or_default()
                .extend(tags.iter().cloned());
        }
        self
    }

    /// Return all items currently in the set (with at least one live tag).
    pub fn items(&self) -> BTreeSet<String> {
        let mut result = BTreeSet::new();
        for (item, tags) in &self.elements {
            let has_live = match self.tombstones.get(item) {
                Some(dead) => tags.iter().any(|t| !dead.contains(t)),
                None => !tags.is_empty(),
            };
            if has_live {
                result.insert(item.clone());
            }
        }
        result
    }

    /// Return the count of currently-present items.
    pub fn len(&self) -> usize {
        self.items().len()
    }

    /// Return `true` if no items are currently present.
    pub fn is_empty(&self) -> bool {
        self.items().is_empty()
    }
}

impl Default for ORSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_contains() {
        let mut s = ORSet::new();
        assert!(!s.contains("a"));
        s.add("a");
        assert!(s.contains("a"));
    }

    #[test]
    fn remove_makes_item_absent() {
        let mut s = ORSet::new();
        s.add("a");
        s.remove("a");
        assert!(!s.contains("a"));
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn add_after_remove_makes_present() {
        let mut s = ORSet::new();
        s.add("a");
        s.remove("a");
        s.add("a");
        assert!(s.contains("a"));
    }

    #[test]
    fn concurrent_add_remove_preserves_add() {
        let mut a = ORSet::new();
        a.add("x");

        // B is a snapshot of A before A's second add.
        let mut b = a.clone();

        // A adds a new tag (concurrent with B's remove).
        a.add("x");

        // B removes "x" (tombstones only the tags B knows about).
        b.remove("x");

        // Merge B into A — A's second tag survives.
        a.merge(&b);
        assert!(a.contains("x"));
    }

    #[test]
    fn merge_unions_elements() {
        let mut a = ORSet::new();
        a.add("x");
        let mut b = ORSet::new();
        b.add("y");
        a.merge(&b);
        assert!(a.contains("x"));
        assert!(a.contains("y"));
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn merge_is_commutative() {
        let mut a = ORSet::new();
        a.add("x");
        let mut b = ORSet::new();
        b.add("y");

        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);

        assert_eq!(ab.items(), ba.items());
    }

    #[test]
    fn remove_nonexistent_is_noop() {
        let mut s = ORSet::new();
        s.remove("nonexistent");
        assert!(!s.contains("nonexistent"));
    }

    #[test]
    fn default_is_empty() {
        let s = ORSet::default();
        assert!(s.is_empty());
    }
}
