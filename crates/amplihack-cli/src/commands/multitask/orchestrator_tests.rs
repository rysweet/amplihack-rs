use super::*;

#[test]
fn test_orchestrator_construction() {
    let orch = ParallelOrchestrator::new("https://github.com/test/repo", "recipe");
    assert_eq!(orch.mode, "recipe");
    assert_eq!(
        orch.default_max_runtime, None,
        "issue #2022: no default runtime limit on a workstream"
    );
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

/// Issue #2022: an explicit runtime limit is still honoured.
#[test]
fn an_explicit_default_max_runtime_is_kept() {
    let mut orch = ParallelOrchestrator::new(".", "recipe");
    orch.set_default_max_runtime(900);
    assert_eq!(orch.default_max_runtime, Some(900));
}

/// Issue #2022: with no `max_runtime` a running workstream is never marked
/// timed out, however long it has run.
#[test]
fn a_workstream_without_max_runtime_is_never_timed_out() {
    use crate::commands::multitask::state::enforce_timeouts;
    let dir = tempfile::tempdir().unwrap();
    let running = |max_runtime: Option<u64>| {
        let mut ws = Workstream::new(
            9,
            "feat/long".into(),
            "d".into(),
            "t".into(),
            "default-workflow".into(),
            dir.path(),
            dir.path(),
        );
        ws.lifecycle_state = "running".into();
        ws.start_time = Some(std::time::Instant::now());
        ws.max_runtime = max_runtime;
        ws
    };
    // Control: a limit already reached is enforced, so the assertion below
    // cannot pass on a no-op.
    let mut all = [running(Some(0))];
    enforce_timeouts(&mut all, &std::collections::HashMap::new());
    assert_eq!(all[0].lifecycle_state, "timed_out_resumable");

    // With no limit, elapsed time is never compared at all.
    let mut all = [running(None)];
    enforce_timeouts(&mut all, &std::collections::HashMap::new());
    assert_eq!(all[0].lifecycle_state, "running");
}
