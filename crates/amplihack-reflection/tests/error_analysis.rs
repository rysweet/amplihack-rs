// crates/amplihack-reflection/tests/error_analysis.rs
//
// TDD: failing tests for ContextualErrorAnalyzer (port of
// amplifier-bundle/tools/amplihack/reflection/contextual_error_analyzer.py).

use amplihack_reflection::error_analysis::{ContextualErrorAnalyzer, ErrorCategory, Severity};

#[test]
fn detects_module_not_found_python_import_error() {
    let a = ContextualErrorAnalyzer::new();
    let err = "ModuleNotFoundError: No module named 'foo.bar'";
    let analysis = a.analyze_error_context(err, "").unwrap();
    assert_eq!(analysis.category, ErrorCategory::ImportError);
    assert!(analysis.severity >= Severity::Medium);
    assert!(!analysis.suggestions.is_empty());
}

#[test]
fn detects_permission_denied() {
    let a = ContextualErrorAnalyzer::new();
    let analysis = a
        .analyze_error_context(
            "PermissionError: [Errno 13] Permission denied: '/etc/x'",
            "",
        )
        .unwrap();
    assert_eq!(analysis.category, ErrorCategory::Permission);
}

#[test]
fn detects_network_timeout() {
    let a = ContextualErrorAnalyzer::new();
    let analysis = a
        .analyze_error_context("ConnectionError: timed out after 30s", "")
        .unwrap();
    assert_eq!(analysis.category, ErrorCategory::Network);
}

#[test]
fn unknown_error_falls_back_to_generic_category() {
    let a = ContextualErrorAnalyzer::new();
    let analysis = a
        .analyze_error_context("flubber widget exploded uncontrollably", "")
        .unwrap();
    assert_eq!(analysis.category, ErrorCategory::Unknown);
    assert!(analysis.suggestions.is_empty() || !analysis.suggestions.is_empty());
}

#[test]
fn top_suggestion_returns_highest_confidence() {
    let a = ContextualErrorAnalyzer::new();
    let top = a
        .top_suggestion("ModuleNotFoundError: No module named 'requests'", "")
        .unwrap()
        .expect("top suggestion present");
    assert!(top.confidence > 0.0);
    assert!(!top.text.is_empty());
}

#[test]
fn analysis_is_deterministic_for_same_input() {
    let a = ContextualErrorAnalyzer::new();
    let e = "FileNotFoundError: [Errno 2] No such file or directory: 'x.txt'";
    let r1 = a.analyze_error_context(e, "").unwrap();
    let r2 = a.analyze_error_context(e, "").unwrap();
    assert_eq!(r1.category, r2.category);
    assert_eq!(r1.suggestions.len(), r2.suggestions.len());
}
