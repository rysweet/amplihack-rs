use super::*;

#[test]
fn test_orchestrator_construction() {
    let orch = ParallelOrchestrator::new("https://github.com/test/repo", "recipe");
    assert_eq!(orch.mode, "recipe");
    assert_eq!(orch.default_max_runtime, DEFAULT_MAX_RUNTIME);
    assert!(orch.workstreams.is_empty());
}

#[test]
fn test_set_timeout_policy() {
    let mut orch = ParallelOrchestrator::new(".", "recipe");
    orch.set_default_timeout_policy("continue-preserve");
    assert_eq!(orch.default_timeout_policy, "continue-preserve");

    orch.set_default_timeout_policy("invalid-policy");
    assert_eq!(orch.default_timeout_policy, "continue-preserve");
}

#[test]
fn default_branch_fallback_is_bounded_and_diagnostic() {
    let source = include_str!("orchestrator.rs");

    assert!(
        source.contains("run_output_with_timeout") || source.contains("run_with_timeout"),
        "git ls-remote default branch resolution must use an explicit timeout helper"
    );
    assert!(
        source.contains("falling back to main") || source.contains("fallback to main"),
        "fallback to main must be observable in diagnostics"
    );
    assert!(
        source.contains("stderr") || source.contains("stdout") || source.contains("diagnostic"),
        "default branch fallback diagnostics should include failure context"
    );
}
