//! Success-criteria evaluation against collected evidence.
//!
//! Parses a criteria string into individual requirements, matches each
//! requirement against evidence and execution logs, awards bonus points
//! for tests and documentation, and produces an [`EvaluationResult`].
//!
//! Ported from the Python `success_evaluator.py`.

use std::collections::HashMap;

use regex::Regex;

use crate::models::{EvaluationResult, EvidenceItem, EvidenceType};

// ---------------------------------------------------------------------------
// Criteria parsing
// ---------------------------------------------------------------------------

/// Parse a success-criteria string into individual requirement strings.
///
/// Accepts bullet lists (`-`, `*`, `•`), numbered lists (`1.`, `2.`), and
/// plain prose lines (longer than 3 words and not ending with `:`).
pub fn parse_success_criteria(criteria: &str) -> Vec<String> {
    let trimmed = criteria.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let bullet_re = Regex::new(r"^[-*•]\s+(.+)$").expect("valid regex");
    let numbered_re = Regex::new(r"^\d+\.\s+(.+)$").expect("valid regex");

    let mut requirements = Vec::new();

    for raw_line in trimmed.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(caps) = bullet_re.captures(line) {
            requirements.push(caps[1].trim().to_string());
        } else if let Some(caps) = numbered_re.captures(line) {
            requirements.push(caps[1].trim().to_string());
        } else if !line.ends_with(':') && line.split_whitespace().count() > 3 {
            requirements.push(line.to_string());
        }
    }
    requirements
}

// ---------------------------------------------------------------------------
// Stop-word set for key-term extraction
// ---------------------------------------------------------------------------

const COMMON_WORDS: &[&str] = &[
    "has", "have", "with", "the", "a", "an", "and", "or", "for", "to", "of", "in", "on", "at",
    "by", "is", "are", "should", "must", "will", "can", "be", "been", "being",
];

fn extract_key_terms(text: &str) -> Vec<String> {
    let word_re = Regex::new(r"\b\w+\b").expect("valid regex");
    word_re
        .find_iter(text)
        .map(|m| m.as_str().to_lowercase())
        .filter(|w| w.len() > 2 && !COMMON_WORDS.contains(&w.as_str()))
        .collect()
}

// ---------------------------------------------------------------------------
// Test / documentation detection helpers
// ---------------------------------------------------------------------------

/// Patterns indicating tests passed (searched case-insensitively).
const PASS_PATTERNS: &[&str] = &[
    "all tests passed",
    "pass",
    "ok",
    "100% passed",
    "tests successful",
    "✓",
    "test passed",
];

/// Returns `true` if the execution log or evidence indicate passing tests.
fn has_passing_tests(evidence: &[EvidenceItem], execution_log: &str) -> bool {
    let log_lower = execution_log.to_lowercase();

    for pattern in PASS_PATTERNS {
        if let Some(pos) = log_lower.find(pattern) {
            // Guard: make sure "fail" doesn't appear nearby.
            let end = (pos + 50).min(log_lower.len());
            if !log_lower[..end].contains("fail") {
                return true;
            }
        }
    }

    // Check test-result evidence files.
    for item in evidence.iter().filter(|e| e.evidence_type == EvidenceType::TestResults) {
        let c = item.content.to_lowercase();
        if c.contains("passed") && c.contains("failed: 0") {
            return true;
        }
    }

    false
}

/// Returns `true` if documentation evidence exists and is substantive (>100 bytes).
fn has_documentation(evidence: &[EvidenceItem]) -> bool {
    evidence
        .iter()
        .filter(|e| {
            matches!(
                e.evidence_type,
                EvidenceType::Documentation | EvidenceType::ArchitectureDoc | EvidenceType::ApiSpec
            )
        })
        .any(|e| e.size_bytes > 100)
}

// ---------------------------------------------------------------------------
// SuccessEvaluator
// ---------------------------------------------------------------------------

/// Evaluates task success against criteria using collected evidence.
///
/// # Usage
/// ```rust
/// use amplihack_delegation::success_evaluator::SuccessEvaluator;
///
/// let evaluator = SuccessEvaluator::new();
/// let result = evaluator.evaluate("- code compiles\n- tests pass", &[], "all tests passed");
/// assert!(result.score > 0);
/// ```
#[derive(Debug, Default)]
pub struct SuccessEvaluator;

impl SuccessEvaluator {
    /// Create a new evaluator.
    pub fn new() -> Self {
        Self
    }

    /// Evaluate `criteria` against `evidence` and `execution_log`.
    ///
    /// Returns an [`EvaluationResult`] with score 0–100, per-requirement
    /// breakdown, and bonus points for tests / documentation.
    pub fn evaluate(
        &self,
        criteria: &str,
        evidence: &[EvidenceItem],
        execution_log: &str,
    ) -> EvaluationResult {
        let requirements = parse_success_criteria(criteria);

        if requirements.is_empty() {
            return self.evaluate_basic(evidence, execution_log);
        }

        let mut met = Vec::new();
        let mut missing = Vec::new();

        for req in &requirements {
            if self.is_requirement_met(req, evidence, execution_log) {
                met.push(req.clone());
            } else {
                missing.push(req.clone());
            }
        }

        let base_score = if requirements.is_empty() {
            50u32
        } else {
            ((met.len() as f64 / requirements.len() as f64) * 100.0) as u32
        };

        let mut bonus: u32 = 0;
        if has_passing_tests(evidence, execution_log) {
            bonus += 10;
        }
        if has_documentation(evidence) {
            bonus += 5;
        }

        let final_score = (base_score + bonus).min(100);
        let notes = self.generate_notes(&met, &missing, evidence, execution_log, bonus);

        EvaluationResult::new(final_score, notes, met, missing, bonus)
    }

    // -- private helpers ----------------------------------------------------

    fn evaluate_basic(&self, evidence: &[EvidenceItem], execution_log: &str) -> EvaluationResult {
        let mut score: u32 = 50;

        let code_count = evidence
            .iter()
            .filter(|e| e.evidence_type == EvidenceType::CodeFile)
            .count();
        let test_count = evidence
            .iter()
            .filter(|e| e.evidence_type == EvidenceType::TestFile)
            .count();
        let doc_count = evidence
            .iter()
            .filter(|e| e.evidence_type == EvidenceType::Documentation)
            .count();

        if code_count > 0 {
            score += 20;
        }
        if test_count > 0 {
            score += 15;
        }
        if doc_count > 0 {
            score += 10;
        }
        if has_passing_tests(evidence, execution_log) {
            score += 5;
        }

        let notes = format!(
            "Basic evaluation: Found {code_count} code files, \
             {test_count} test files, {doc_count} documentation files."
        );

        EvaluationResult::new(score.min(100), notes, vec![], vec![], 0)
    }

    fn is_requirement_met(
        &self,
        requirement: &str,
        evidence: &[EvidenceItem],
        execution_log: &str,
    ) -> bool {
        let key_terms = extract_key_terms(&requirement.to_lowercase());
        if key_terms.is_empty() {
            return false;
        }

        let evidence_text: String = evidence
            .iter()
            .map(|e| format!("{} {}", e.path, e.excerpt))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();

        let log_lower = execution_log.to_lowercase();

        let matches = key_terms
            .iter()
            .filter(|t| evidence_text.contains(t.as_str()) || log_lower.contains(t.as_str()))
            .count();

        matches as f64 / key_terms.len() as f64 >= 0.5
    }

    fn generate_notes(
        &self,
        met: &[String],
        missing: &[String],
        evidence: &[EvidenceItem],
        execution_log: &str,
        bonus: u32,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();

        let total = met.len() + missing.len();
        if total > 0 {
            parts.push(format!("Requirements satisfied: {}/{total}", met.len()));
        }

        if !met.is_empty() {
            parts.push("\n✓ Requirements met:".into());
            for req in met.iter().take(5) {
                parts.push(format!("  - {req}"));
            }
            if met.len() > 5 {
                parts.push(format!("  ... and {} more", met.len() - 5));
            }
        }

        if !missing.is_empty() {
            parts.push("\n✗ Requirements not found or incomplete:".into());
            for req in missing.iter().take(5) {
                parts.push(format!("  - {req}"));
            }
            if missing.len() > 5 {
                parts.push(format!("  ... and {} more", missing.len() - 5));
            }
        }

        if bonus > 0 {
            parts.push(format!("\n✓ Bonus points: +{bonus}"));
            if has_passing_tests(evidence, execution_log) {
                parts.push("  - Passing tests detected".into());
            }
            if has_documentation(evidence) {
                parts.push("  - Documentation provided".into());
            }
        }

        // Evidence summary.
        let mut by_type: HashMap<String, usize> = HashMap::new();
        for item in evidence {
            *by_type.entry(item.evidence_type.to_string()).or_insert(0) += 1;
        }
        if !by_type.is_empty() {
            parts.push("\nEvidence collected:".into());
            let mut sorted: Vec<_> = by_type.into_iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            for (t, c) in sorted {
                parts.push(format!("  - {t}: {c}"));
            }
        }

        parts.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_criteria() {
        assert!(parse_success_criteria("").is_empty());
        assert!(parse_success_criteria("   ").is_empty());
    }

    #[test]
    fn parse_bullet_list() {
        let c = "- Code compiles\n- Tests pass\n- Docs exist";
        let r = parse_success_criteria(c);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0], "Code compiles");
    }

    #[test]
    fn parse_numbered_list() {
        let c = "1. First requirement here\n2. Second requirement here";
        let r = parse_success_criteria(c);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn parse_skips_headers() {
        let c = "Requirements:\n- Actual requirement here";
        let r = parse_success_criteria(c);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0], "Actual requirement here");
    }

    #[test]
    fn extract_key_terms_filters_common() {
        let terms = extract_key_terms("the code should have tests");
        assert!(!terms.contains(&"the".to_string()));
        assert!(!terms.contains(&"should".to_string()));
        assert!(terms.contains(&"code".to_string()));
        assert!(terms.contains(&"tests".to_string()));
    }

    #[test]
    fn evaluator_basic_no_criteria() {
        let eval = SuccessEvaluator::new();
        let r = eval.evaluate("", &[], "");
        assert_eq!(r.score, 50);
        assert!(r.notes.contains("Basic evaluation"));
    }
}
