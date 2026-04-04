//! Fact lifecycle management: TTL, confidence decay, and garbage collection.
//!
//! Facts in the hive mind decay over time unless refreshed. This module
//! provides time-based confidence decay (exponential) and garbage collection
//! of expired facts.
//!
//! - Pure functions where possible — easy to test, no hidden state.
//! - Standard library only.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::graph::HiveGraph;

// ---------------------------------------------------------------------------
// Constants (mirroring Python constants.py)
// ---------------------------------------------------------------------------

/// Default fact time-to-live: 24 hours in seconds.
pub const DEFAULT_FACT_TTL_SECONDS: f64 = 86_400.0;

/// Default exponential decay rate per hour.
pub const DEFAULT_CONFIDENCE_DECAY_RATE: f64 = 0.01;

/// Default maximum age before garbage collection (hours).
pub const DEFAULT_MAX_AGE_HOURS: f64 = 24.0;

/// Seconds per hour.
pub const SECONDS_PER_HOUR: f64 = 3_600.0;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Time-to-live metadata for a hive fact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FactTTL {
    pub fact_id: String,
    /// Unix timestamp when the fact was created or last refreshed.
    pub created_at: f64,
    /// Maximum lifetime in seconds.
    pub ttl_seconds: f64,
    /// Exponential decay rate per hour.
    pub confidence_decay_rate: f64,
}

impl FactTTL {
    /// Create a new TTL entry using the given timestamp as creation time.
    pub fn new(fact_id: impl Into<String>, now: f64) -> Self {
        Self {
            fact_id: fact_id.into(),
            created_at: now,
            ttl_seconds: DEFAULT_FACT_TTL_SECONDS,
            confidence_decay_rate: DEFAULT_CONFIDENCE_DECAY_RATE,
        }
    }
}

// ---------------------------------------------------------------------------
// Pure functions
// ---------------------------------------------------------------------------

/// Compute decayed confidence using exponential decay.
///
/// `confidence_new = confidence_original × exp(−decay_rate × elapsed_hours)`
///
/// Result is clamped to \[0.0, 1.0\].
pub fn decay_confidence(original_confidence: f64, elapsed_hours: f64, decay_rate: f64) -> f64 {
    if elapsed_hours <= 0.0 {
        return original_confidence.clamp(0.0, 1.0);
    }
    (original_confidence * (-decay_rate * elapsed_hours).exp()).clamp(0.0, 1.0)
}

/// Garbage-collect expired facts from a [`HiveGraph`].
///
/// Removes facts whose age exceeds `max_age_hours` and deletes their
/// TTL entries from the registry.
///
/// Returns the list of fact IDs that were removed.
pub fn gc_expired_facts(
    hive: &mut HiveGraph,
    ttl_registry: &mut HashMap<String, FactTTL>,
    max_age_hours: f64,
    now: f64,
) -> Vec<String> {
    let max_age_seconds = max_age_hours * SECONDS_PER_HOUR;
    let mut removed = Vec::new();

    let fact_ids: Vec<String> = ttl_registry.keys().cloned().collect();
    for fact_id in fact_ids {
        let age_seconds = now - ttl_registry[&fact_id].created_at;
        if age_seconds >= max_age_seconds {
            let _ = hive.remove_fact(&fact_id);
            ttl_registry.remove(&fact_id);
            removed.push(fact_id);
        }
    }
    removed
}

/// Refresh a fact's confidence and reset its TTL timer.
///
/// Returns `true` if the fact was found and refreshed, `false` otherwise.
pub fn refresh_confidence(
    hive: &mut HiveGraph,
    ttl_registry: &mut HashMap<String, FactTTL>,
    fact_id: &str,
    new_confidence: f64,
    now: f64,
) -> bool {
    if !hive.set_fact_confidence(fact_id, new_confidence) {
        return false;
    }

    if let Some(ttl) = ttl_registry.get_mut(fact_id) {
        ttl.created_at = now;
    } else {
        ttl_registry.insert(fact_id.to_string(), FactTTL::new(fact_id, now));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- decay_confidence ---------------------------------------------------

    #[test]
    fn no_decay_at_zero_hours() {
        assert!((decay_confidence(0.9, 0.0, 0.01) - 0.9).abs() < 1e-9);
    }

    #[test]
    fn decay_reduces_confidence() {
        let decayed = decay_confidence(1.0, 10.0, 0.01);
        assert!(decayed < 1.0);
        assert!(decayed > 0.0);
    }

    #[test]
    fn high_decay_rate_drops_fast() {
        let slow = decay_confidence(1.0, 5.0, 0.01);
        let fast = decay_confidence(1.0, 5.0, 1.0);
        assert!(fast < slow);
    }

    #[test]
    fn decay_clamps_to_unit_range() {
        assert!(decay_confidence(1.5, 0.0, 0.01) <= 1.0);
        assert!(decay_confidence(-0.5, 0.0, 0.01) >= 0.0);
    }

    #[test]
    fn negative_elapsed_returns_clamped_original() {
        assert!((decay_confidence(0.8, -5.0, 0.01) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn decay_formula_matches_expected() {
        // confidence_new = 1.0 * exp(-0.01 * 24) ≈ 0.7866
        let decayed = decay_confidence(1.0, 24.0, 0.01);
        assert!((decayed - 0.7866).abs() < 0.001);
    }

    // -- gc_expired_facts ---------------------------------------------------

    #[test]
    fn gc_removes_expired_facts() {
        let mut hive = HiveGraph::new();
        let id = hive
            .store_fact("concept", "old content", 0.8, "agent-1", vec![])
            .unwrap();

        let mut registry = HashMap::new();
        registry.insert(
            id.clone(),
            FactTTL {
                fact_id: id.clone(),
                created_at: 0.0,
                ttl_seconds: DEFAULT_FACT_TTL_SECONDS,
                confidence_decay_rate: DEFAULT_CONFIDENCE_DECAY_RATE,
            },
        );

        // 25 hours later
        let now = 25.0 * SECONDS_PER_HOUR;
        let removed = gc_expired_facts(&mut hive, &mut registry, DEFAULT_MAX_AGE_HOURS, now);

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0], id);
        assert!(registry.is_empty());
        assert_eq!(hive.fact_count(), 0);
    }

    #[test]
    fn gc_keeps_fresh_facts() {
        let mut hive = HiveGraph::new();
        let id = hive
            .store_fact("concept", "fresh content", 0.8, "agent-1", vec![])
            .unwrap();

        let mut registry = HashMap::new();
        let now = 1000.0;
        registry.insert(id.clone(), FactTTL::new(&id, now));

        // Only 1 hour later
        let removed = gc_expired_facts(
            &mut hive,
            &mut registry,
            DEFAULT_MAX_AGE_HOURS,
            now + 3600.0,
        );

        assert!(removed.is_empty());
        assert_eq!(hive.fact_count(), 1);
    }

    // -- refresh_confidence -------------------------------------------------

    #[test]
    fn refresh_updates_confidence() {
        let mut hive = HiveGraph::new();
        let id = hive
            .store_fact("concept", "content", 0.5, "agent-1", vec![])
            .unwrap();

        let mut registry = HashMap::new();
        registry.insert(id.clone(), FactTTL::new(&id, 100.0));

        assert!(refresh_confidence(
            &mut hive,
            &mut registry,
            &id,
            0.9,
            200.0
        ));

        let fact = hive.get_fact(&id).unwrap().unwrap();
        assert!((fact.confidence - 0.9).abs() < 1e-9);
        assert!((registry[&id].created_at - 200.0).abs() < 1e-9);
    }

    #[test]
    fn refresh_missing_fact_returns_false() {
        let mut hive = HiveGraph::new();
        let mut registry = HashMap::new();
        assert!(!refresh_confidence(
            &mut hive,
            &mut registry,
            "nonexistent",
            0.9,
            100.0,
        ));
    }

    #[test]
    fn refresh_creates_ttl_if_absent() {
        let mut hive = HiveGraph::new();
        let id = hive
            .store_fact("concept", "content", 0.5, "agent-1", vec![])
            .unwrap();

        let mut registry = HashMap::new();
        assert!(refresh_confidence(
            &mut hive,
            &mut registry,
            &id,
            0.7,
            300.0
        ));
        assert!(registry.contains_key(&id));
        assert!((registry[&id].created_at - 300.0).abs() < 1e-9);
    }

    #[test]
    fn refresh_clamps_confidence() {
        let mut hive = HiveGraph::new();
        let id = hive
            .store_fact("concept", "content", 0.5, "agent-1", vec![])
            .unwrap();
        let mut registry = HashMap::new();

        refresh_confidence(&mut hive, &mut registry, &id, 1.5, 100.0);
        let fact = hive.get_fact(&id).unwrap().unwrap();
        assert!((fact.confidence - 1.0).abs() < 1e-9);
    }
}
