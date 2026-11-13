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
fn default_branch_uses_the_repository_head() {
    let directory = tempfile::tempdir().unwrap();
    let mut init = amplihack_git::command();
    assert!(
        init.args(["init", "--initial-branch=trunk"])
            .arg(directory.path())
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(directory.path().join("file"), "fixture").unwrap();
    let mut add = amplihack_git::command();
    assert!(
        add.current_dir(directory.path())
            .args(["add", "file"])
            .status()
            .unwrap()
            .success()
    );
    let mut commit = amplihack_git::command();
    assert!(
        commit
            .current_dir(directory.path())
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "fixture"
            ])
            .status()
            .unwrap()
            .success()
    );
    let mut orch = ParallelOrchestrator::new(directory.path().to_str().unwrap(), "recipe");
    assert_eq!(orch.resolve_default_branch(), "trunk");
}
