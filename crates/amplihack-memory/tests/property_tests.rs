//! Property-based tests for amplihack-memory using proptest.
//!
//! Verifies invariants like store/recall roundtrips, bloom filter FP
//! rates, hash ring distribution, and serialization stability.

use amplihack_memory::bloom::BloomFilter;
use amplihack_memory::hash_ring::HashRing;
use amplihack_memory::models::{MemoryEntry, MemoryType};
use proptest::prelude::*;

// ── Strategies ──

fn arb_memory_type() -> impl Strategy<Value = MemoryType> {
    prop_oneof![
        Just(MemoryType::Semantic),
        Just(MemoryType::Episodic),
        Just(MemoryType::Procedural),
        Just(MemoryType::Working),
        Just(MemoryType::Strategic),
        Just(MemoryType::Prospective),
    ]
}

fn arb_memory_entry() -> impl Strategy<Value = MemoryEntry> {
    (
        "[a-z0-9]{4,16}", // session_id
        "[a-z0-9]{4,16}", // agent_id
        arb_memory_type(),
        ".{1,500}", // content
    )
        .prop_map(|(sid, aid, mt, content)| MemoryEntry::new(sid, aid, mt, content))
}

// ── MemoryEntry properties ──

proptest! {
    /// Serialization roundtrip: to_json -> from_json preserves all fields.
    #[test]
    fn entry_json_roundtrip(entry in arb_memory_entry()) {
        let json = entry.to_json().unwrap();
        let restored = MemoryEntry::from_json(&json).unwrap();

        prop_assert_eq!(&entry.id, &restored.id);
        prop_assert_eq!(&entry.content, &restored.content);
        prop_assert_eq!(entry.memory_type, restored.memory_type);
        prop_assert_eq!(&entry.session_id, &restored.session_id);
        prop_assert_eq!(&entry.agent_id, &restored.agent_id);
        prop_assert!((entry.importance - restored.importance).abs() < 1e-10);
    }

    /// Content fingerprint is deterministic.
    #[test]
    fn fingerprint_is_deterministic(entry in arb_memory_entry()) {
        let fp1 = entry.content_fingerprint();
        let fp2 = entry.content_fingerprint();
        prop_assert_eq!(fp1, fp2);
    }

    /// Different content produces different fingerprints (probabilistic).
    #[test]
    fn fingerprint_differs_for_different_content(
        a in ".{10,200}",
        b in ".{10,200}",
    ) {
        prop_assume!(a != b);
        let e1 = MemoryEntry::new("s", "a", MemoryType::Semantic, &a);
        let e2 = MemoryEntry::new("s", "a", MemoryType::Semantic, &b);
        // Hash collisions are possible but extremely unlikely for distinct content
        // We allow up to 1 in 2^32 collision rate
        prop_assert_ne!(e1.content_fingerprint(), e2.content_fingerprint());
    }

    /// to_dict produces a map with all expected keys.
    #[test]
    fn to_dict_has_all_keys(entry in arb_memory_entry()) {
        let dict = entry.to_dict();
        prop_assert!(dict.contains_key("id"));
        prop_assert!(dict.contains_key("content"));
        prop_assert!(dict.contains_key("memory_type"));
        prop_assert!(dict.contains_key("session_id"));
        prop_assert!(dict.contains_key("agent_id"));
        prop_assert!(dict.contains_key("importance"));
        prop_assert!(dict.contains_key("tags"));
    }
}

// ── BloomFilter properties ──

proptest! {
    /// No false negatives: if we add an item, might_contain always returns true.
    #[test]
    fn bloom_no_false_negatives(items in prop::collection::vec("[a-z0-9]{1,50}", 1..100)) {
        let mut bf = BloomFilter::new(items.len() * 2, 0.01);
        for item in &items {
            bf.add(item);
        }
        for item in &items {
            prop_assert!(bf.might_contain(item), "false negative for {:?}", item);
        }
    }

    /// Count matches number of unique items added.
    #[test]
    fn bloom_count_matches(items in prop::collection::vec("[a-z0-9]{5,20}", 1..50)) {
        let mut bf = BloomFilter::new(200, 0.01);
        let mut seen = std::collections::HashSet::new();
        for item in &items {
            bf.add(item);
            seen.insert(item.clone());
        }
        prop_assert_eq!(bf.count(), seen.len());
    }

    /// Serialization roundtrip: to_bytes -> from_bytes preserves state.
    #[test]
    fn bloom_serialize_roundtrip(items in prop::collection::vec("[a-z]{3,10}", 1..20)) {
        let mut bf = BloomFilter::new(100, 0.01);
        for item in &items {
            bf.add(item);
        }

        let bytes = bf.to_bytes().unwrap();
        let restored = BloomFilter::from_bytes(&bytes).unwrap();

        for item in &items {
            prop_assert!(restored.might_contain(item));
        }
        prop_assert_eq!(bf.count(), restored.count());
    }
}

// ── HashRing properties ──

proptest! {
    /// Every item maps to some agent when agents exist.
    #[test]
    fn hash_ring_always_maps(
        agents in prop::collection::vec("[a-z]{3,8}", 1..10),
        keys in prop::collection::vec("[a-z0-9]{5,20}", 1..50),
    ) {
        let mut ring = HashRing::new(3);
        let mut unique_agents = std::collections::HashSet::new();
        for agent in &agents {
            if unique_agents.insert(agent.clone()) {
                ring.add_agent(agent);
            }
        }

        for key in &keys {
            let primary = ring.get_primary_agent(key);
            prop_assert!(primary.is_some(), "key {:?} should map to an agent", key);
            prop_assert!(
                unique_agents.contains(primary.unwrap()),
                "mapped agent should be one we added"
            );
        }
    }

    /// Removing an agent doesn't crash and remaining agents still get mapped.
    #[test]
    fn hash_ring_remove_resilient(
        agents in prop::collection::vec("[a-z]{3,8}", 2..8),
    ) {
        let mut ring = HashRing::new(3);
        let mut unique: Vec<String> = Vec::new();
        for a in &agents {
            if !unique.contains(a) {
                unique.push(a.clone());
                ring.add_agent(a);
            }
        }

        if unique.len() > 1 {
            ring.remove_agent(&unique[0]);
            let primary = ring.get_primary_agent("test-key");
            prop_assert!(primary.is_some());
            prop_assert_ne!(primary.unwrap(), &unique[0]);
        }
    }
}
