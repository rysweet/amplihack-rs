//! Distributed Hash Table (DHT) for agent-centric fact sharding.
//!
//! Each agent owns a range of the consistent hash ring.  Facts are hashed
//! to positions on the ring and stored on the R nearest agents (replication
//! factor).  Queries route to shard owners via ring lookup.

mod ring;
mod router;
mod store;

pub use ring::HashRing;
pub use router::DHTRouter;
pub use store::{ShardFact, ShardStore};

/// Number of virtual nodes per agent for even distribution.
pub const VIRTUAL_NODES_PER_AGENT: usize = 64;
/// Default replication factor.
pub const DEFAULT_REPLICATION_FACTOR: usize = 3;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Hash a string key to a 32-bit ring position using FNV-1a.
pub(crate) fn hash_key(key: &str) -> u32 {
    const FNV_OFFSET: u32 = 2_166_136_261;
    const FNV_PRIME: u32 = 16_777_619;
    let mut hash = FNV_OFFSET;
    for byte in key.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Stop words filtered when building a content routing key.
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "that", "with", "this", "from", "are", "was", "were", "has", "have",
    "had", "not", "but",
];

/// Generate a stable routing key from fact content.
///
/// Uses the first 5 significant words (skipping stop words and short tokens)
/// as the routing key for consistent hash placement.
pub(crate) fn content_key(content: &str) -> String {
    let words: Vec<&str> = content
        .split_whitespace()
        .filter(|w| w.len() > 2 && !STOP_WORDS.contains(&w.to_lowercase().as_str()))
        .take(5)
        .collect();

    if words.is_empty() {
        let end = content.len().min(20);
        content[..end].to_string()
    } else {
        words
            .iter()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_key_is_deterministic() {
        assert_eq!(hash_key("hello"), hash_key("hello"));
        assert_ne!(hash_key("hello"), hash_key("world"));
    }

    #[test]
    fn content_key_filters_stop_words() {
        let key = content_key("the quick brown fox jumps over the lazy dog");
        assert!(!key.contains("the"));
        assert!(key.contains("quick"));
    }

    #[test]
    fn content_key_fallback_for_short_content() {
        let key = content_key("ab");
        assert_eq!(key, "ab");
    }
}
