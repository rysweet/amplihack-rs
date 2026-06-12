//! crates/amplihack-utils/src/tests/artifact_guard_tests.rs
//!
//! Contracts for issue #755 Artifact Guard.
//!
//! These tests define the reusable library contract:
//! scan staged, tracked, untracked, and ignored-present generated/runtime
//! artifacts; fail closed on unsafe allowlists; report violations without
//! deleting, moving, unstaging, or rewriting files.

use super::{
    ArtifactAllowlist, ArtifactGuardConfig, ArtifactGuardMode, ArtifactSource, ArtifactViolation,
    scan_artifacts,
};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

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

fn repo() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    run_git(tmp.path(), &["init", "-q"]);
    run_git(
        tmp.path(),
        &["config", "user.email", "artifact-guard@example.invalid"],
    );
    run_git(tmp.path(), &["config", "user.name", "Artifact Guard Test"]);
    write_file(&tmp.path().join("README.md"), "# fixture\n");
    run_git(tmp.path(), &["add", "README.md"]);
    run_git(tmp.path(), &["commit", "-qm", "initial"]);
    tmp
}

fn default_config(repo: &Path) -> ArtifactGuardConfig {
    ArtifactGuardConfig::new(repo).with_mode(ArtifactGuardMode::PreCommit)
}

fn violation_for<'a>(
    violations: &'a [ArtifactViolation],
    path: &str,
    source: ArtifactSource,
) -> &'a ArtifactViolation {
    violations
        .iter()
        .find(|violation| violation.path == path && violation.source == source)
        .unwrap_or_else(|| {
            panic!("missing violation path={path} source={source:?}; got {violations:#?}")
        })
}

#[test]
fn staged_node_modules_are_blocked_before_broad_staging_or_commit() {
    let tmp = repo();
    write_file(
        &tmp.path().join("node_modules/leak/index.js"),
        "module.exports = 1;\n",
    );
    run_git(tmp.path(), &["add", "-f", "node_modules/leak/index.js"]);

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    assert!(report.has_violations(), "staged node_modules must block");
    let violation = violation_for(
        &report.violations,
        "node_modules/leak/index.js",
        ArtifactSource::Staged,
    );
    assert_eq!(violation.rule_id, "node-modules");
    assert!(
        violation.remediation.contains("remove")
            && violation
                .remediation
                .contains("outside the parent worktree"),
        "violation must include clear remediation, got: {}",
        violation.remediation
    );
    assert!(
        tmp.path().join("node_modules/leak/index.js").exists(),
        "guard must not delete artifacts"
    );
}

#[test]
fn ignored_present_dist_plugin_runtime_and_cache_artifacts_are_blocked() {
    let tmp = repo();
    write_file(
        &tmp.path().join(".gitignore"),
        "dist/plugin.js\n.claude/runtime/\n.next/cache/\n",
    );
    run_git(tmp.path(), &["add", ".gitignore"]);
    run_git(tmp.path(), &["commit", "-qm", "ignore generated outputs"]);

    write_file(
        &tmp.path().join("dist/plugin.js"),
        "generated plugin bundle\n",
    );
    write_file(&tmp.path().join(".claude/runtime/session.json"), "{}\n");
    write_file(&tmp.path().join(".next/cache/webpack/index.bin"), "cache\n");

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    for (path, rule_id) in [
        ("dist/plugin.js", "plugin-bundle"),
        (".claude/runtime/session.json", "claude-runtime"),
        (".next/cache/webpack/index.bin", "cache-artifact"),
    ] {
        let violation = violation_for(&report.violations, path, ArtifactSource::IgnoredPresent);
        assert_eq!(
            violation.rule_id, rule_id,
            "{path} must be classified by the expected rule"
        );
    }
}

#[test]
fn ignored_present_workflow_session_artifacts_are_blocked() {
    let tmp = repo();
    write_file(
        &tmp.path().join(".gitignore"),
        "recipe-runner.log\nplan.md\n.copilot/session-state/\n.amplihack/session-state/\n",
    );
    run_git(tmp.path(), &["add", ".gitignore"]);
    run_git(
        tmp.path(),
        &["commit", "-qm", "ignore workflow runtime outputs"],
    );

    write_file(&tmp.path().join("recipe-runner.log"), "recipe output\n");
    write_file(&tmp.path().join("plan.md"), "temporary plan\n");
    write_file(
        &tmp.path()
            .join(".copilot/session-state/session-123/files/output.json"),
        "{}\n",
    );
    write_file(
        &tmp.path()
            .join(".amplihack/session-state/session-123/workflow.json"),
        "{}\n",
    );

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PrePublish),
    )
    .expect("scan artifacts");

    for path in [
        "recipe-runner.log",
        "plan.md",
        ".copilot/session-state/session-123/files/output.json",
        ".amplihack/session-state/session-123/workflow.json",
    ] {
        let violation = violation_for(&report.violations, path, ArtifactSource::IgnoredPresent);
        assert_eq!(
            violation.rule_id, "workflow-session-artifact",
            "{path} must be classified as a workflow/session artifact"
        );
    }
}

#[test]
fn nested_ignored_present_artifacts_are_blocked() {
    let tmp = repo();
    write_file(
        &tmp.path().join(".gitignore"),
        "frontend/node_modules/\npackages/app/dist/\n",
    );
    run_git(tmp.path(), &["add", ".gitignore"]);
    run_git(
        tmp.path(),
        &["commit", "-qm", "ignore nested generated outputs"],
    );

    write_file(
        &tmp.path().join("frontend/node_modules/leak/package.json"),
        "{}\n",
    );
    write_file(
        &tmp.path().join("packages/app/dist/assets/app.js"),
        "bundle\n",
    );

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    violation_for(
        &report.violations,
        "frontend/node_modules/leak/package.json",
        ArtifactSource::IgnoredPresent,
    );
    violation_for(
        &report.violations,
        "packages/app/dist/assets/app.js",
        ArtifactSource::IgnoredPresent,
    );
}

#[test]
fn narrow_allowlist_entry_does_not_hide_sibling_ignored_artifacts() {
    let tmp = repo();
    write_file(&tmp.path().join(".gitignore"), "dist/\n");
    write_file(
        &tmp.path().join(".amplihack-artifact-allowlist"),
        "# reviewed generated fixture\ndist/plugin.js\n",
    );
    run_git(tmp.path(), &["add", ".gitignore"]);
    run_git(tmp.path(), &["commit", "-qm", "ignore dist output"]);

    write_file(&tmp.path().join("dist/plugin.js"), "intentional fixture\n");
    write_file(&tmp.path().join("dist/zz-leak.bin"), "leak\n");

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    assert!(
        !report
            .violations
            .iter()
            .any(|violation| violation.path == "dist/plugin.js"),
        "exact allowlist entry must suppress only dist/plugin.js"
    );
    violation_for(
        &report.violations,
        "dist/zz-leak.bin",
        ArtifactSource::IgnoredPresent,
    );
}

#[test]
fn untracked_nested_worktrees_and_build_artifacts_are_blocked() {
    let tmp = repo();
    write_file(
        &tmp.path().join("worktrees/feature/README.md"),
        "nested worktree\n",
    );
    write_file(&tmp.path().join("coverage/lcov.info"), "TN:\n");

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    violation_for(
        &report.violations,
        "worktrees/feature/README.md",
        ArtifactSource::Untracked,
    );
    violation_for(
        &report.violations,
        "coverage/lcov.info",
        ArtifactSource::Untracked,
    );
}

#[test]
fn normal_source_files_and_ignored_rust_target_do_not_block_local_development() {
    let tmp = repo();
    write_file(&tmp.path().join("src/lib.rs"), "pub fn ok() {}\n");
    write_file(&tmp.path().join(".gitignore"), "target/\n");
    write_file(
        &tmp.path().join("target/debug/deps/libfixture.rlib"),
        "build output\n",
    );

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    assert!(
        report.is_clean(),
        "ordinary source files and ignored Rust target/ output should not block; got {:#?}",
        report.violations
    );
}

#[test]
fn tracked_generated_artifacts_are_blocked_even_when_not_staged() {
    let tmp = repo();
    write_file(
        &tmp.path().join("dist/plugin.js"),
        "committed generated bundle\n",
    );
    run_git(tmp.path(), &["add", "-f", "dist/plugin.js"]);
    run_git(
        tmp.path(),
        &["commit", "-qm", "accidentally commit plugin bundle"],
    );

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    violation_for(
        &report.violations,
        "dist/plugin.js",
        ArtifactSource::Tracked,
    );
}

#[test]
fn staged_deletion_of_tracked_generated_artifact_allows_cleanup_commit() {
    let tmp = repo();
    write_file(
        &tmp.path().join("dist/plugin.js"),
        "committed generated bundle\n",
    );
    run_git(tmp.path(), &["add", "-f", "dist/plugin.js"]);
    run_git(
        tmp.path(),
        &["commit", "-qm", "accidentally commit plugin bundle"],
    );
    run_git(tmp.path(), &["rm", "-q", "dist/plugin.js"]);

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    assert!(
        report.is_clean(),
        "staged deletion cleanup commits should not be blocked; got {:#?}",
        report.violations
    );
}

#[test]
fn narrow_allowlist_entry_exempts_only_the_exact_documented_artifact() {
    let tmp = repo();
    write_file(
        &tmp.path().join(".amplihack-artifact-allowlist"),
        "# Security-reviewed fixture for issue #755 tests.\ndist/plugin.js\n",
    );
    write_file(
        &tmp.path().join("dist/plugin.js"),
        "intentional checked fixture\n",
    );
    write_file(&tmp.path().join("node_modules/leak/index.js"), "leak\n");

    let config =
        default_config(tmp.path()).with_allowlist(tmp.path().join(".amplihack-artifact-allowlist"));
    let report = scan_artifacts(&config).expect("scan artifacts");

    assert!(
        !report
            .violations
            .iter()
            .any(|violation| violation.path == "dist/plugin.js"),
        "exact allowlist entry must suppress only dist/plugin.js"
    );
    violation_for(
        &report.violations,
        "node_modules/leak/index.js",
        ArtifactSource::Untracked,
    );
}

#[test]
fn allowlist_accepts_comments_blank_lines_and_narrow_entries() {
    let tmp = repo();
    let allowlist = tmp.path().join(".amplihack-artifact-allowlist");
    write_file(
        &allowlist,
        "\n# reviewed fixture exception\n\ndocs/fixtures/plugin-output/dist/plugin.js\n",
    );

    let loaded = ArtifactAllowlist::load(&allowlist).expect("load allowlist");

    assert!(
        loaded.is_allowed("docs/fixtures/plugin-output/dist/plugin.js"),
        "narrow reviewed entries must be active"
    );
}

#[test]
fn allowlist_rejects_absolute_parent_traversing_empty_or_broad_entries() {
    let tmp = repo();
    let allowlist = tmp.path().join(".amplihack-artifact-allowlist");

    for entry in [
        "/tmp/dist/plugin.js",
        "../dist/plugin.js",
        "node_modules/",
        "node_modules/**",
        "dist/**",
        "recipe-runner.log",
        "plan.md",
        ".copilot/session-state/**",
        ".amplihack/session-state/**",
        ".claude/runtime/**",
        "worktrees/**",
    ] {
        write_file(&allowlist, &format!("{entry}\n"));
        let error = ArtifactAllowlist::load(&allowlist)
            .expect_err("unsafe allowlist entry must fail closed");
        let message = error.to_string();
        assert!(
            message.contains("allowlist") && message.contains("unsafe"),
            "unsafe entry `{entry}` must produce a clear allowlist error; got: {message}"
        );
    }
}

#[test]
fn allowlist_with_no_reviewed_entries_fails_closed() {
    let tmp = repo();
    let allowlist = tmp.path().join(".amplihack-artifact-allowlist");
    write_file(&allowlist, "\n# comments only\n");

    let error = ArtifactAllowlist::load(&allowlist).expect_err("empty allowlists must fail closed");

    assert!(
        error.to_string().contains("allowlist") && error.to_string().contains("unsafe"),
        "comments-only allowlist must be rejected clearly; got: {error}"
    );
}

#[test]
fn repo_path_must_resolve_inside_a_git_worktree() {
    let tmp = TempDir::new().expect("tempdir");

    let error = scan_artifacts(&default_config(tmp.path()))
        .expect_err("non-git directories must fail closed");

    let message = error.to_string();
    assert!(
        message.contains("git") && message.contains("worktree"),
        "non-git repo errors must be explicit; got: {message}"
    );
}
