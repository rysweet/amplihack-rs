//! Helper functions and constants for the self-improvement loop.

/// Failure taxonomy categories matching Python's error_analyzer.
pub(crate) const FAILURE_CATEGORIES: &[(&str, &[&str])] = &[
    (
        "retrieval_insufficient",
        &["not found", "missing", "no result", "empty"],
    ),
    (
        "temporal_ordering_wrong",
        &["before", "after", "order", "sequence", "first"],
    ),
    (
        "intent_misclassification",
        &["intent", "classify", "misunderstood", "wrong type"],
    ),
    (
        "fact_extraction_incomplete",
        &["partial", "incomplete", "missed", "omitted"],
    ),
    (
        "synthesis_hallucination",
        &["hallucinate", "fabricate", "invented", "false"],
    ),
    (
        "update_not_applied",
        &["update", "stale", "old version", "not applied"],
    ),
    (
        "contradiction_undetected",
        &["contradict", "conflict", "inconsistent"],
    ),
    (
        "procedural_ordering_lost",
        &["step", "procedure", "order lost", "sequence"],
    ),
    (
        "teaching_coverage_gap",
        &["teach", "coverage", "gap", "not covered"],
    ),
    (
        "counterfactual_refusal",
        &["counterfactual", "hypothetical", "what if"],
    ),
];

/// Classify a failure message into a taxonomy category.
pub(crate) fn classify_failure(error_lower: &str) -> (&'static str, String) {
    for &(category, keywords) in FAILURE_CATEGORIES {
        if keywords.iter().any(|kw| error_lower.contains(kw)) {
            return (category, format!("Matched pattern in error: {error_lower}"));
        }
    }
    ("unknown", format!("Unclassified failure: {error_lower}"))
}
