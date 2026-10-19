use super::source_fixture::NativeFixture;

#[test]
fn canonical_native_ordinary_target_checkpoint_commits_staged_changes() {
    let f = NativeFixture::new();
    for variant in ["success", "hook-config"] {
        let r = f.checkpoint(variant);
        assert_eq!(r.code, 0, "{}", r.diagnostics);
        assert_eq!(r.child_code, 0);
        assert_eq!(r.output["success"], true);
        assert_eq!(
            r.output["step_results"][0]["step_id"],
            "checkpoint-after-implementation"
        );
        assert!(r.diagnostics.contains("Artifact Guard clean"));
        assert!(r.diagnostics.contains("Checkpoint commit created"));
        assert_ne!(r.before, r.after);
        assert!(r.staged.is_empty());
        assert_eq!(
            r.hook.as_deref(),
            Some(if variant == "hook-config" {
                "unset"
            } else {
                "1"
            })
        );
        assert_eq!(
            r.subject.trim(),
            "wip: checkpoint after implementation (steps 7-8)"
        );
    }
}

#[test]
fn canonical_native_checkpoint_preserves_real_hook_failure_and_staged_change() {
    let r = NativeFixture::new().checkpoint("hook-failure");
    assert_eq!(r.code, 1, "{}", r.diagnostics);
    assert_eq!(r.child_code, 1);
    assert_eq!(r.output["success"], false);
    assert!(r.diagnostics.contains("NATIVE_CHECKPOINT_HOOK_REJECTED"));
    assert!(
        r.diagnostics
            .contains("git commit failed during checkpoint-after-implementation (exit 1)")
    );
    assert_eq!(r.before, r.after);
    assert!(r.staged.contains("tracked.txt"));
    assert_eq!(r.hook.as_deref(), Some("1"));
}

#[test]
fn canonical_native_checkpoint_rejects_missing_framework_commit_helper() {
    let r = NativeFixture::new().checkpoint("missing-helper");
    assert_ne!(r.code, 0);
    assert_eq!(r.child_code, 2);
    assert_eq!(r.output["success"], false);
    assert!(r.diagnostics.contains("workflow_checkpoint_commit.sh"));
    assert_eq!(r.before, r.after);
    assert!(r.staged.contains("tracked.txt"));
    assert_eq!(r.hook, None);
}

#[test]
fn canonical_native_checkpoint_keeps_hygiene_guard_authoritative() {
    let r = NativeFixture::new().checkpoint("hygiene-block");
    assert_eq!(r.code, 1, "{}", r.diagnostics);
    assert_eq!(r.child_code, 1);
    assert_eq!(r.output["success"], false);
    assert!(r.diagnostics.contains("Artifact Guard blocked"));
    assert_eq!(r.before, r.after);
    assert!(r.staged.contains("plan.md"));
    assert_eq!(r.hook, None);
}
