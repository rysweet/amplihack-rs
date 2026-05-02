//! Semantic duplicate detection (port of `semantic_duplicate_detector.py`).
//!
//! The Python module called the Claude SDK with a `difflib` fallback. The
//! Rust port keeps the deterministic similarity fallback only.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateDetectionResult {
    pub is_duplicate: bool,
    pub similar_issues: Vec<serde_json::Value>,
    pub confidence: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Copy)]
pub struct SemanticDuplicateDetector {
    /// Similarity threshold above which content is considered a duplicate.
    pub threshold: f64,
}

impl Default for SemanticDuplicateDetector {
    fn default() -> Self {
        Self { threshold: 0.75 }
    }
}

impl SemanticDuplicateDetector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn similarity(&self, a: &str, b: &str) -> f64 {
        if a.is_empty() && b.is_empty() {
            return 1.0;
        }
        let toks_a = tokenise(a);
        let toks_b = tokenise(b);
        if toks_a.is_empty() || toks_b.is_empty() {
            return 0.0;
        }
        let mut shared = 0usize;
        for tok in &toks_a {
            if toks_b.contains(tok) {
                shared += 1;
            }
        }
        // Jaccard-style similarity over deduped token sets.
        let union: std::collections::HashSet<&String> =
            toks_a.iter().chain(toks_b.iter()).collect();
        if union.is_empty() {
            0.0
        } else {
            shared as f64 / union.len() as f64
        }
    }

    pub fn compare(&self, new_content: &str, existing: &str) -> DuplicateDetectionResult {
        let score = self.similarity(new_content, existing);
        DuplicateDetectionResult {
            is_duplicate: score >= self.threshold,
            similar_issues: vec![],
            confidence: score,
            reason: if score >= self.threshold {
                format!(
                    "Token similarity {score:.2} ≥ threshold {:.2}",
                    self.threshold
                )
            } else {
                format!(
                    "Token similarity {score:.2} below threshold {:.2}",
                    self.threshold
                )
            },
        }
    }
}

fn tokenise(s: &str) -> Vec<String> {
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && w.len() > 2)
        .map(|w| w.to_string())
        .collect()
}
