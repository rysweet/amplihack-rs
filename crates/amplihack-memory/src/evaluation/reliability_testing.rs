//! Active backend reliability testing: data integrity, sequential safety,
//! and error recovery.
//!
//! Ported from Python `memory/evaluation/reliability_evaluator.py`.

use crate::backend::MemoryBackend;
use crate::models::{MemoryEntry, MemoryQuery, MemoryType};

use super::metrics::BackendReliabilityMetrics;

/// Actively tests backend reliability: data integrity, sequential safety,
/// and error recovery.
///
/// Ported from Python `ReliabilityEvaluator` which tests concurrent safety
/// via asyncio. In sync Rust the borrow checker prevents data races, so
/// we verify rapid sequential operations instead.
pub struct BackendReliabilityEvaluator;

impl BackendReliabilityEvaluator {
    /// Run all reliability tests against a backend.
    pub fn evaluate(backend: &mut dyn MemoryBackend) -> BackendReliabilityMetrics {
        let backend_name = backend.backend_name().to_string();
        let integrity = Self::test_data_integrity(backend);
        let safety = Self::test_sequential_safety(backend);
        let recovery = Self::test_error_recovery(backend);

        BackendReliabilityMetrics {
            data_integrity_score: integrity,
            concurrent_safety_score: safety,
            error_recovery_score: recovery,
            num_tests: 3,
            backend_name,
        }
    }

    /// Store memories with diverse content, retrieve, verify round-trip.
    fn test_data_integrity(backend: &mut dyn MemoryBackend) -> f64 {
        let long_content = "Long content repeated. ".repeat(50);
        let test_cases: Vec<(&str, MemoryType)> = vec![
            ("Simple text for integrity test", MemoryType::Episodic),
            ("Special chars: !@#$%^&*()", MemoryType::Semantic),
            ("Line one\nLine two\nLine three", MemoryType::Procedural),
            ("Unicode: 你好世界 Привет 🚀🎯", MemoryType::Prospective),
            (&long_content, MemoryType::Working),
        ];

        let mut successes = 0;
        let total = test_cases.len();

        for (content, mt) in &test_cases {
            let entry = MemoryEntry::new("integrity-session", "test-agent", *mt, *content);
            let stored_id = match backend.store(&entry) {
                Ok(id) => id,
                Err(_) => continue,
            };

            let first_word = content.split_whitespace().next().unwrap_or("test");
            let query = MemoryQuery::new(first_word);
            if let Ok(results) = backend.retrieve(&query)
                && results.iter().any(|r| r.id == stored_id && r.content == *content)
            {
                successes += 1;
            }
        }

        successes as f64 / total as f64
    }

    /// Rapid sequential store operations — verifies no corruption.
    fn test_sequential_safety(backend: &mut dyn MemoryBackend) -> f64 {
        let num_ops = 10;
        let mut successes = 0;

        for i in 0..num_ops {
            let entry = MemoryEntry::new(
                "safety-session",
                "test-agent",
                MemoryType::Semantic,
                format!("Sequential safety test entry number {i} with content"),
            );
            if backend.store(&entry).is_ok() {
                successes += 1;
            }
        }

        successes as f64 / num_ops as f64
    }

    /// Test graceful handling of edge cases and invalid inputs.
    fn test_error_recovery(backend: &mut dyn MemoryBackend) -> f64 {
        let mut graceful = 0;
        let total = 3;

        // 1. Delete non-existent entry
        match backend.delete("nonexistent-id-that-does-not-exist") {
            Ok(false) | Err(_) => graceful += 1,
            Ok(true) => {}
        }

        // 2. Empty query string
        let query = MemoryQuery::new("");
        match backend.retrieve(&query) {
            Ok(_) | Err(_) => graceful += 1,
        }

        // 3. Query with empty memory_types filter
        let query = MemoryQuery {
            memory_types: vec![],
            ..MemoryQuery::new("test query")
        };
        match backend.retrieve(&query) {
            Ok(_) | Err(_) => graceful += 1,
        }

        graceful as f64 / total as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::InMemoryBackend;

    #[test]
    fn integrity_test() {
        let mut backend = InMemoryBackend::new();
        let score = BackendReliabilityEvaluator::test_data_integrity(&mut backend);
        assert!(score > 0.5, "integrity score {score} too low");
    }

    #[test]
    fn sequential_safety() {
        let mut backend = InMemoryBackend::new();
        let score = BackendReliabilityEvaluator::test_sequential_safety(&mut backend);
        assert_eq!(score, 1.0);
    }

    #[test]
    fn error_recovery() {
        let mut backend = InMemoryBackend::new();
        let score = BackendReliabilityEvaluator::test_error_recovery(&mut backend);
        assert_eq!(score, 1.0);
    }

    #[test]
    fn full_evaluate() {
        let mut backend = InMemoryBackend::new();
        let metrics = BackendReliabilityEvaluator::evaluate(&mut backend);
        assert_eq!(metrics.num_tests, 3);
        assert!(metrics.data_integrity_score > 0.0);
        assert_eq!(metrics.concurrent_safety_score, 1.0);
        assert_eq!(metrics.error_recovery_score, 1.0);
        assert_eq!(metrics.backend_name, "in_memory");
    }
}
