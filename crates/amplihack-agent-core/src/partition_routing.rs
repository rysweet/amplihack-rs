//! Deterministic Event Hubs partition routing.
//!
//! Port of Python `partition_routing.py` — provides stable cross-process
//! partition selection using SHA-256 for non-numeric agent IDs while
//! preserving the fast numeric path for the common `agent-N` naming convention.

use sha2::{Digest, Sha256};

/// Default number of Event Hub partitions.
pub const DEFAULT_EVENT_HUB_PARTITIONS: u32 = 32;

/// Return a deterministic numeric index for an agent identifier.
///
/// For identifiers following the `agent-N` pattern (or any `prefix-N`), the
/// trailing numeric portion is returned directly. For all other identifiers a
/// SHA-256 hash is used to produce a stable 64-bit index.
///
/// # Examples
///
/// ```
/// use amplihack_agent_core::partition_routing::stable_agent_index;
///
/// assert_eq!(stable_agent_index("agent-3"), 3);
/// assert_eq!(stable_agent_index("agent-0"), 0);
///
/// // Non-numeric suffix falls back to SHA-256-based index.
/// let idx = stable_agent_index("my-special-agent");
/// assert!(idx > 0);
/// ```
pub fn stable_agent_index(agent_id: &str) -> u64 {
    if let Some(suffix) = agent_id.rsplit('-').next()
        && let Ok(n) = suffix.parse::<u64>()
    {
        return n;
    }
    let hash = Sha256::digest(agent_id.as_bytes());
    let bytes: [u8; 8] = hash[..8].try_into().expect("sha256 always >= 8 bytes");
    u64::from_be_bytes(bytes)
}

/// Compute the partition index for the given agent in a hub with
/// `num_partitions` partitions.
pub fn partition_for_agent(agent_id: &str, num_partitions: u32) -> u32 {
    (stable_agent_index(agent_id) % u64::from(num_partitions)) as u32
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_suffix_fast_path() {
        assert_eq!(stable_agent_index("agent-0"), 0);
        assert_eq!(stable_agent_index("agent-42"), 42);
        assert_eq!(stable_agent_index("worker-100"), 100);
    }

    #[test]
    fn non_numeric_suffix_hashes() {
        let idx = stable_agent_index("my-special-agent");
        // Should be deterministic across runs.
        assert_eq!(idx, stable_agent_index("my-special-agent"));
    }

    #[test]
    fn different_ids_different_hashes() {
        let a = stable_agent_index("alpha");
        let b = stable_agent_index("beta");
        assert_ne!(a, b);
    }

    #[test]
    fn partition_within_range() {
        for id in &["agent-0", "agent-5", "my-agent", "x"] {
            let p = partition_for_agent(id, DEFAULT_EVENT_HUB_PARTITIONS);
            assert!(p < DEFAULT_EVENT_HUB_PARTITIONS, "partition {p} out of range");
        }
    }

    #[test]
    fn partition_deterministic() {
        let a = partition_for_agent("test-agent", 16);
        let b = partition_for_agent("test-agent", 16);
        assert_eq!(a, b);
    }

    #[test]
    fn default_partitions_constant() {
        assert_eq!(DEFAULT_EVENT_HUB_PARTITIONS, 32);
    }
}
