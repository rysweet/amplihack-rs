//! TDD contract tests for PR #579 recovery readiness.
//!
//! These tests are intentionally written before the implementation. They define
//! the required public contract for recovering an existing PR without manually
//! merging it, bypassing merge protection, launching nested `default-workflow`
//! instances, or claiming a no-op against the wrong head.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use amplihack_cli::pr_recovery_readiness::{
    AdditiveCopyEntry, AdditiveCopyPlan, CheckConclusion, CheckRollup, CheckStatus,
    HookReadinessInput, HookRegistration, InstalledHook, MergeState, NoOpReportInput,
    PrHeadSnapshot, inspect_additive_copy_plan, inspect_hook_readiness, render_no_op_report,
    verify_pr_head,
};

const PR_579_HEAD: &str = "8fb46865fb4412038b9313a62c02cc5aa0693132";

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn amplihack_bin() -> &'static str {
    env!("CARGO_BIN_EXE_amplihack")
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", repo.display()));
    assert!(
        output.status.success(),
        "git {args:?} failed in {}\nstdout:\n{}\nstderr:\n{}",
        repo.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
    }
    fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn init_repo(repo: &Path) {
    run_git(repo, &["init", "-q", "-b", "main"]);
    run_git(
        repo,
        &["config", "user.email", "runtime-artifacts@example.invalid"],
    );
    run_git(repo, &["config", "user.name", "Runtime Artifact Test"]);
    write_file(&repo.join("README.md"), "# fixture\n");
    write_file(
        &repo.join(".claude/settings.json"),
        "{\"user\":\"owned\"}\n",
    );
    run_git(repo, &["add", "README.md", ".claude/settings.json"]);
    run_git(repo, &["commit", "-qm", "initial"]);
}

#[test]
fn head_verifier_accepts_only_exact_expected_local_and_pr_head_match() {
    let snapshot = PrHeadSnapshot {
        expected_head_sha: PR_579_HEAD.to_string(),
        local_head_sha: PR_579_HEAD.to_string(),
        pr_head_sha: PR_579_HEAD.to_string(),
    };

    let verified = verify_pr_head(&snapshot).expect("exact three-way head match must pass");

    assert_eq!(verified.head_sha, PR_579_HEAD);
    assert!(
        verified
            .message
            .contains("local HEAD == PR headRefOid == expected_head_sha")
    );
}

#[test]
fn head_verifier_blocks_no_op_when_local_head_differs_from_expected_head() {
    let snapshot = PrHeadSnapshot {
        expected_head_sha: PR_579_HEAD.to_string(),
        local_head_sha: "1111111111111111111111111111111111111111".to_string(),
        pr_head_sha: PR_579_HEAD.to_string(),
    };

    let error = verify_pr_head(&snapshot).expect_err("local HEAD mismatch must fail closed");
    let message = error.to_string();

    assert!(message.contains("local HEAD"));
    assert!(message.contains(PR_579_HEAD));
    assert!(message.contains("1111111111111111111111111111111111111111"));
    assert!(message.contains("blocked"));
}

#[test]
fn head_verifier_blocks_no_op_when_pr_head_differs_from_expected_head() {
    let snapshot = PrHeadSnapshot {
        expected_head_sha: PR_579_HEAD.to_string(),
        local_head_sha: PR_579_HEAD.to_string(),
        pr_head_sha: "2222222222222222222222222222222222222222".to_string(),
    };

    let error = verify_pr_head(&snapshot).expect_err("PR head mismatch must fail closed");
    let message = error.to_string();

    assert!(message.contains("PR headRefOid"));
    assert!(message.contains(PR_579_HEAD));
    assert!(message.contains("2222222222222222222222222222222222222222"));
    assert!(message.contains("blocked"));
}

#[test]
fn hook_readiness_accepts_complete_copilot_and_default_workflow_hook_wiring() {
    let input = HookReadinessInput {
        installed_hooks: vec![
            InstalledHook::new("amplihack", "PreToolUse.js"),
            InstalledHook::new("amplihack", "PostToolUse.js"),
            InstalledHook::new("amplihack", "Stop.js"),
            InstalledHook::new("amplihack", "SessionStart.js"),
            InstalledHook::new("amplihack", "SessionStop.js"),
            InstalledHook::new("amplihack", "UserPromptSubmit.js"),
            InstalledHook::new("amplihack", "PreCompact.js"),
            InstalledHook::new("xpia", "PreToolUse.js"),
        ],
        native_registrations: vec![
            HookRegistration::new("PreToolUse", "amplihack-hooks pre-tool-use"),
            HookRegistration::new("PostToolUse", "amplihack-hooks post-tool-use"),
            HookRegistration::new("Stop", "amplihack-hooks stop"),
            HookRegistration::new("SessionStart", "amplihack-hooks session-start"),
            HookRegistration::new("SessionStop", "amplihack-hooks session-stop"),
            HookRegistration::new(
                "UserPromptSubmit",
                "amplihack-hooks workflow-classification-reminder",
            ),
            HookRegistration::new("UserPromptSubmit", "amplihack-hooks user-prompt-submit"),
            HookRegistration::new("PreCompact", "amplihack-hooks pre-compact"),
        ],
    };

    let readiness = inspect_hook_readiness(&input);

    assert!(
        readiness.blockers.is_empty(),
        "complete hook wiring must not produce blockers: {:?}",
        readiness.blockers
    );
    assert!(readiness.workflow_ready);
}

#[test]
fn hook_readiness_reports_missing_workflow_classification_registration_as_blocker() {
    let input = HookReadinessInput {
        installed_hooks: vec![
            InstalledHook::new("amplihack", "PreToolUse.js"),
            InstalledHook::new("amplihack", "PostToolUse.js"),
            InstalledHook::new("amplihack", "Stop.js"),
            InstalledHook::new("amplihack", "SessionStart.js"),
            InstalledHook::new("amplihack", "SessionStop.js"),
            InstalledHook::new("amplihack", "UserPromptSubmit.js"),
            InstalledHook::new("amplihack", "PreCompact.js"),
            InstalledHook::new("xpia", "PreToolUse.js"),
        ],
        native_registrations: vec![
            HookRegistration::new("PreToolUse", "amplihack-hooks pre-tool-use"),
            HookRegistration::new("PostToolUse", "amplihack-hooks post-tool-use"),
            HookRegistration::new("Stop", "amplihack-hooks stop"),
            HookRegistration::new("SessionStart", "amplihack-hooks session-start"),
            HookRegistration::new("SessionStop", "amplihack-hooks session-stop"),
            HookRegistration::new("UserPromptSubmit", "amplihack-hooks user-prompt-submit"),
            HookRegistration::new("PreCompact", "amplihack-hooks pre-compact"),
        ],
    };

    let readiness = inspect_hook_readiness(&input);

    assert!(!readiness.workflow_ready);
    assert!(
        readiness
            .blockers
            .iter()
            .any(|b| b.code == "MISSING_NATIVE_HOOK_REGISTRATION"
                && b.message.contains("workflow-classification-reminder")),
        "missing workflow classification registration must be explicit: {:?}",
        readiness.blockers
    );
}

#[test]
fn additive_copy_plan_preserves_existing_user_owned_files() {
    let plan = AdditiveCopyPlan {
        destination_root: PathBuf::from("/home/user/.amplihack"),
        entries: vec![
            AdditiveCopyEntry::file("amplifier-bundle/CLAUDE.md", true),
            AdditiveCopyEntry::file("amplifier-bundle/recipes/default-workflow.yaml", false),
        ],
    };

    let readiness = inspect_additive_copy_plan(&plan).expect("safe relative paths should inspect");

    assert!(readiness.workflow_ready);
    assert_eq!(
        readiness.action_for("amplifier-bundle/CLAUDE.md"),
        Some("skip-existing"),
        "existing user-owned files must be skipped, not overwritten"
    );
    assert_eq!(
        readiness.action_for("amplifier-bundle/recipes/default-workflow.yaml"),
        Some("copy-new")
    );
    assert!(
        readiness
            .actions
            .iter()
            .all(|a| a != "delete" && a != "overwrite-existing"),
        "additive copy must never delete or overwrite existing user-owned files: {:?}",
        readiness.actions
    );
}

#[test]
fn additive_copy_plan_rejects_absolute_paths_and_parent_directory_escapes() {
    for entry in [
        AdditiveCopyEntry::file("/tmp/escape.txt", false),
        AdditiveCopyEntry::file("../escape.txt", false),
        AdditiveCopyEntry::file("amplifier-bundle/../../escape.txt", false),
    ] {
        let plan = AdditiveCopyPlan {
            destination_root: PathBuf::from("/home/user/.amplihack"),
            entries: vec![entry],
        };

        let error = inspect_additive_copy_plan(&plan).expect_err("unsafe path must be rejected");
        let message = error.to_string();
        assert!(message.contains("path traversal") || message.contains("absolute path"));
    }
}

#[test]
fn noop_report_allows_workflow_ready_while_test_is_in_progress_and_merge_is_blocked() {
    let report = render_no_op_report(NoOpReportInput {
        head: PrHeadSnapshot {
            expected_head_sha: PR_579_HEAD.to_string(),
            local_head_sha: PR_579_HEAD.to_string(),
            pr_head_sha: PR_579_HEAD.to_string(),
        },
        checks: vec![
            CheckRollup::new(
                "Lint & Format",
                CheckStatus::Completed,
                CheckConclusion::Success,
            ),
            CheckRollup::new("build", CheckStatus::Completed, CheckConclusion::Success),
            CheckRollup::new("Test", CheckStatus::InProgress, CheckConclusion::Pending),
        ],
        merge_state: MergeState::Blocked,
        hook_ready: true,
        additive_copy_ready: true,
        files_modified: vec![],
        manual_merge_performed: false,
        merge_bypass_performed: false,
        nested_default_workflow_launched: false,
    })
    .expect("exact-head no-op report with pending Test should be workflow-ready");

    assert!(report.workflow_ready);
    assert!(!report.merge_ready);
    assert_eq!(report.head_sha, PR_579_HEAD);
    assert!(report.no_op_justification.contains(PR_579_HEAD));
    assert!(report.no_op_justification.contains("Lint/Format green"));
    assert!(report.no_op_justification.contains("builds green"));
    assert!(report.no_op_justification.contains("Test in progress"));
    assert!(report.no_op_justification.contains("merge blocked"));
    assert!(!report.manual_merge_performed);
    assert!(!report.merge_bypass_performed);
    assert!(!report.nested_default_workflow_launched);
}

#[test]
fn noop_report_rejects_manual_merge_bypass_or_nested_default_workflow() {
    for (manual_merge, merge_bypass, nested_default_workflow) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let error = render_no_op_report(NoOpReportInput {
            head: PrHeadSnapshot {
                expected_head_sha: PR_579_HEAD.to_string(),
                local_head_sha: PR_579_HEAD.to_string(),
                pr_head_sha: PR_579_HEAD.to_string(),
            },
            checks: vec![
                CheckRollup::new(
                    "Lint & Format",
                    CheckStatus::Completed,
                    CheckConclusion::Success,
                ),
                CheckRollup::new("build", CheckStatus::Completed, CheckConclusion::Success),
                CheckRollup::new("Test", CheckStatus::InProgress, CheckConclusion::Pending),
            ],
            merge_state: MergeState::Blocked,
            hook_ready: true,
            additive_copy_ready: true,
            files_modified: vec![],
            manual_merge_performed: manual_merge,
            merge_bypass_performed: merge_bypass,
            nested_default_workflow_launched: nested_default_workflow,
        })
        .expect_err("prohibited recovery path must block no-op report");

        let message = error.to_string();
        assert!(
            message.contains("manual merge")
                || message.contains("merge bypass")
                || message.contains("nested default-workflow"),
            "error must name the prohibited recovery path: {message}"
        );
    }
}

#[test]
fn recovery_runtime_artifact_preflight_removes_known_self_violations_but_guard_stays_strict() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).expect("create repo dir");
    init_repo(&repo);

    write_file(
        &repo.join(".claude/runtime/logs/recovery-session.log"),
        "workflow-generated runtime log\n",
    );
    write_file(
        &repo.join("worktrees/nested-default-workflow/trace.log"),
        "workflow-generated nested worktree output\n",
    );
    write_file(
        &repo.join("dist/plugin.js"),
        "unrelated generated artifact\n",
    );

    let helper = workspace_root().join("amplifier-bundle/tools/workflow_runtime_artifacts.sh");
    let script = format!(
        r#"set -euo pipefail
source "{}"
preflight_known_workflow_runtime_artifacts "{}"
"#,
        helper.display(),
        repo.display()
    );
    let preflight = Command::new("bash")
        .arg("-c")
        .arg(script)
        .output()
        .expect("run runtime artifact preflight");

    assert!(
        preflight.status.success(),
        "preflight must remove only known workflow runtime artifacts\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&preflight.stdout),
        String::from_utf8_lossy(&preflight.stderr)
    );
    assert!(
        !repo.join(".claude/runtime").exists(),
        "known workflow-generated .claude/runtime must be removed before recovery guard points"
    );
    assert!(
        !repo.join("worktrees").exists(),
        "workflow-created nested worktrees must be removed before recovery guard points"
    );
    assert!(
        repo.join(".claude/settings.json").exists(),
        "user-authored .claude configuration must survive runtime cleanup"
    );
    assert!(
        repo.join("dist/plugin.js").exists(),
        "unrelated untracked artifacts must not be hidden by runtime cleanup"
    );

    let guard = Command::new(amplihack_bin())
        .args([
            "hygiene",
            "artifact-guard",
            "--repo",
            repo.to_str().expect("utf-8 repo path"),
            "--mode",
            "pre-publish",
        ])
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("run Artifact Guard after runtime preflight");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&guard.stdout),
        String::from_utf8_lossy(&guard.stderr)
    );

    assert_eq!(
        guard.status.code(),
        Some(1),
        "Artifact Guard must remain strict for unrelated generated artifacts\n{combined}"
    );
    assert!(
        combined.contains("dist/plugin.js"),
        "strict guard failure must name the unrelated artifact that cleanup intentionally preserved: {combined}"
    );
    assert!(
        !combined.contains(".claude/runtime")
            && !combined.contains("worktrees/nested-default-workflow"),
        "known workflow runtime artifacts should be removed before Artifact Guard runs: {combined}"
    );
}

#[test]
fn recovery_publish_and_final_status_helpers_preflight_before_dirty_checks() {
    for helper_name in ["workflow_publish_pr.sh", "workflow_final_status.sh"] {
        let helper_path = workspace_root()
            .join("amplifier-bundle")
            .join("tools")
            .join(helper_name);
        let content = fs::read_to_string(&helper_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", helper_path.display()));
        let preflight = content
            .find("preflight_known_workflow_runtime_artifacts")
            .unwrap_or_else(|| {
                panic!("{helper_name} must preflight known workflow runtime artifacts")
            });
        let dirty_check = content
            .find("git status --porcelain")
            .or_else(|| content.find("git diff --quiet"))
            .unwrap_or_else(|| panic!("{helper_name} must contain a dirty-worktree check"));

        assert!(
            content.contains("workflow_runtime_artifacts.sh"),
            "{helper_name} must source the narrow runtime-artifact helper"
        );
        assert!(
            preflight < dirty_check,
            "{helper_name} must preflight known runtime artifacts before dirty-worktree checks"
        );
        assert!(
            !content.contains("rm -rf .claude/runtime") && !content.contains("rm -rf worktrees"),
            "{helper_name} must not inline broad deletion; cleanup belongs in workflow_runtime_artifacts.sh"
        );
    }
}
