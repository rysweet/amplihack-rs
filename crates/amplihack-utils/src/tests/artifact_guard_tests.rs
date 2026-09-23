//! crates/amplihack-utils/src/tests/artifact_guard_tests.rs
//!
//! Contracts for issue #755 Artifact Guard.
//!
//! These tests define the reusable library contract:
//! scan staged, tracked, untracked, and ignored-present generated/runtime
//! artifacts; fail closed on unsafe allowlists; report violations without
//! deleting, moving, unstaging, or rewriting files.

use super::{
    ArtifactAllowlist, ArtifactGuardConfig, ArtifactGuardMode, ArtifactProvenance, ArtifactSource,
    ArtifactViolation, PreExistingPolicy, scan_artifacts,
};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn run_git(repo: &Path, args: &[&str]) {
    let output = amplihack_git::command()
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
    // `All` scans staged + worktree (tracked/untracked) + ignored-present, so it
    // exercises every source in one pass. The pre-commit/pre-publish narrowing
    // for issue #928 (ignored-present is intentionally NOT scanned there because
    // gitignored+present paths can never be committed/published) is asserted by
    // the dedicated `*_pre_commit` / `*_pre_publish` tests below.
    ArtifactGuardConfig::new(repo).with_mode(ArtifactGuardMode::All)
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
        (".next/cache/webpack/index.bin", "cache-artifact"),
    ] {
        let violation = violation_for(&report.violations, path, ArtifactSource::IgnoredPresent);
        assert_eq!(
            violation.rule_id, rule_id,
            "{path} must be classified by the expected rule"
        );
    }
    // The entire `.claude/runtime/` subtree is tool-generated launcher/agent
    // bookkeeping and must never be flagged, even when ignored-but-present.
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.path == ".claude/runtime/session.json"),
        ".claude/runtime/session.json must be exempt; got {:#?}",
        report.violations
    );
}

#[test]
fn claude_runtime_subtree_is_exempt_when_untracked_or_ignored() {
    // Regression for issue #807 and the recurrence that blocked
    // `.claude/runtime/metrics/post_tool_use_metrics.jsonl`: the amplihack
    // launcher, session tracker, and every agent's PostToolUse metrics hook
    // write bookkeeping into `<repo>/.claude/runtime/` continuously while a
    // recipe runs. That output is unavoidable, gitignored, tool-generated
    // runtime state, so the guard must never flag it as untracked or
    // ignored-present — otherwise the end-of-run guard step hard-fails and
    // discards already-committed work.
    let tmp = repo();
    write_file(&tmp.path().join(".gitignore"), ".claude/runtime/\n");
    run_git(tmp.path(), &["add", ".gitignore"]);
    run_git(tmp.path(), &["commit", "-qm", "ignore claude runtime"]);

    let runtime_paths = [
        ".claude/runtime/launcher_context.json",
        ".claude/runtime/sessions.jsonl",
        ".claude/runtime/session.json",
        ".claude/runtime/metrics/tool.json",
        ".claude/runtime/metrics/post_tool_use_metrics.jsonl",
        ".claude/runtime/locks/power_steering.lock",
    ];
    for rel in runtime_paths {
        write_file(&tmp.path().join(rel), "{}\n");
    }

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    for rel in runtime_paths {
        assert!(
            !report.violations.iter().any(|v| v.path == rel),
            "{rel} under .claude/runtime/ must be exempt when untracked/ignored; got {:#?}",
            report.violations
        );
    }
}

#[test]
fn staged_or_tracked_claude_runtime_state_is_still_blocked() {
    // The subtree exemption covers only the untracked/ignored output recipes
    // unavoidably produce. Deliberately committing runtime state into the
    // published tree is genuine pollution and must still fail the guard — except
    // the two launcher-owned bookkeeping files, which are exempt in all sources.
    let tmp = repo();
    write_file(&tmp.path().join(".gitignore"), ".claude/runtime/\n");
    run_git(tmp.path(), &["add", ".gitignore"]);
    run_git(tmp.path(), &["commit", "-qm", "ignore claude runtime"]);

    // Force-stage a non-launcher runtime file past .gitignore.
    write_file(
        &tmp.path()
            .join(".claude/runtime/metrics/post_tool_use_metrics.jsonl"),
        "{}\n",
    );
    run_git(
        tmp.path(),
        &[
            "add",
            "-f",
            ".claude/runtime/metrics/post_tool_use_metrics.jsonl",
        ],
    );
    // Launcher-owned file, also force-staged: exempt in all sources.
    write_file(
        &tmp.path().join(".claude/runtime/launcher_context.json"),
        "{}\n",
    );
    run_git(
        tmp.path(),
        &["add", "-f", ".claude/runtime/launcher_context.json"],
    );

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PrePublish),
    )
    .expect("scan artifacts");

    let blocked = violation_for(
        &report.violations,
        ".claude/runtime/metrics/post_tool_use_metrics.jsonl",
        ArtifactSource::Staged,
    );
    assert_eq!(
        blocked.rule_id, "claude-runtime",
        "staged runtime state must still be blocked as claude-runtime"
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.path == ".claude/runtime/launcher_context.json"),
        "launcher-owned bookkeeping must stay exempt even when staged; got {:#?}",
        report.violations
    );
}

#[test]
fn launcher_context_is_exempt_regardless_of_git_source() {
    // The exemption is path-based and applies whether the launcher file is
    // staged, untracked, or ignored-present — the end-of-run guard runs in
    // `pre-publish` mode and must stay clean in every case.
    let tmp = repo();

    // Staged (force-added past .gitignore).
    write_file(
        &tmp.path().join(".claude/runtime/launcher_context.json"),
        "{}\n",
    );
    run_git(
        tmp.path(),
        &["add", "-f", ".claude/runtime/launcher_context.json"],
    );

    // Untracked sessions log.
    write_file(&tmp.path().join(".claude/runtime/sessions.jsonl"), "{}\n");

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PrePublish),
    )
    .expect("scan artifacts");

    assert!(
        report.is_clean(),
        "launcher-owned runtime files must not be flagged in any source; got {:#?}",
        report.violations
    );
    assert!(
        tmp.path()
            .join(".claude/runtime/launcher_context.json")
            .exists(),
        "guard must not delete launcher state"
    );
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
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::Worktree),
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
fn ignored_present_inside_registered_sibling_worktree_is_not_flagged() {
    // Issue #857: concurrent recipe runs create dedicated task worktrees under
    // `<repo>/worktrees/`. When `worktrees/` is gitignored (as in Simard), each
    // sibling's ignored files (target/, .pytest_cache, .claude/runtime) surface
    // via `git ls-files --others --ignored` and were wrongly flagged as
    // `nested-worktree` leaks, failing every concurrent recipe's finalize.
    let tmp = repo();
    write_file(&tmp.path().join(".gitignore"), "worktrees/\n");
    run_git(tmp.path(), &["add", ".gitignore"]);
    run_git(tmp.path(), &["commit", "-qm", "ignore worktrees"]);

    // A legitimately-registered sibling task worktree.
    run_git(
        tmp.path(),
        &[
            "worktree",
            "add",
            "-q",
            "worktrees/sibling",
            "-b",
            "sibling",
        ],
    );
    // Ignored-present artifacts INSIDE the registered sibling — must be exempt.
    write_file(
        &tmp.path()
            .join("worktrees/sibling/.pytest_cache/CACHEDIR.TAG"),
        "x\n",
    );
    write_file(
        &tmp.path().join("worktrees/sibling/target/.rustc_info.json"),
        "{}\n",
    );
    // A genuinely-leaked (UNregistered) directory under worktrees/ — still flagged.
    write_file(
        &tmp.path().join("worktrees/leaked/target/.rustc_info.json"),
        "{}\n",
    );

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.path.starts_with("worktrees/sibling")),
        "registered sibling worktree wrongly flagged (issue #857): {:?}",
        report.violations
    );
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.path.starts_with("worktrees/leaked")),
        "leaked unregistered worktree must still be flagged: {:?}",
        report.violations
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
        "dist/*",
        "dist/**",
        "build/*",
        "recipe-runner.log",
        "plan.md",
        "*.log",
        ".copilot/session-state/*",
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
fn explicit_allowlist_path_must_stay_inside_repo() {
    let tmp = repo();
    let outside = TempDir::new().expect("outside tempdir");
    let outside_allowlist = outside.path().join("artifact-allowlist");
    write_file(&outside_allowlist, "dist/plugin.js\n");

    let error = scan_artifacts(&default_config(tmp.path()).with_allowlist(outside_allowlist))
        .expect_err("allowlists outside the repository must fail closed");

    let message = error.to_string();
    assert!(
        message.contains("allowlist") && message.contains("outside repository root"),
        "outside-repo allowlist must be rejected clearly; got: {message}"
    );
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

// ---------------------------------------------------------------------------
// Issue #928: pre-commit / pre-publish must NOT fail-closed on ignored-present
// cache artifacts.
//
// Gitignored + untracked cache directories (e.g. `.pytest_cache/`,
// `node_modules/` when gitignored) can never be committed or published, so the
// `pre-commit` and `pre-publish` guard modes must not block on them. Only
// artifacts that *could* actually be committed/published (staged, tracked, and
// untracked-but-not-ignored) may fail those modes. The full-worktree audit
// modes (`worktree`, `all`) must still surface ignored-present artifacts.
// ---------------------------------------------------------------------------

/// Seed a repo whose `.gitignore` ignores the given cache directories, commit
/// the ignore rules, then materialise the cache dirs as untracked + present.
fn repo_with_ignored_present_cache(entries: &[&str]) -> TempDir {
    let tmp = repo();
    let gitignore: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
    write_file(&tmp.path().join(".gitignore"), &gitignore);
    run_git(tmp.path(), &["add", ".gitignore"]);
    run_git(tmp.path(), &["commit", "-qm", "ignore cache artifacts"]);

    // `.pytest_cache/` and `node_modules/` materialised as gitignored + present.
    write_file(
        &tmp.path().join(".pytest_cache/CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    );
    write_file(&tmp.path().join(".pytest_cache/v/cache/lastfailed"), "{}\n");
    write_file(&tmp.path().join("node_modules/.package-lock.json"), "{}\n");
    write_file(
        &tmp.path().join("node_modules/leftpad/index.js"),
        "module.exports = 1;\n",
    );
    tmp
}

#[test]
fn ignored_present_cache_artifacts_do_not_block_pre_commit() {
    let tmp = repo_with_ignored_present_cache(&[".pytest_cache/", "node_modules/"]);

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PreCommit),
    )
    .expect("scan artifacts");

    assert!(
        report.is_clean(),
        "gitignored+present cache artifacts can never be committed and must not \
         block pre-commit (issue #928); got {:#?}",
        report.violations
    );
    assert!(
        tmp.path().join(".pytest_cache/CACHEDIR.TAG").exists()
            && tmp.path().join("node_modules/leftpad/index.js").exists(),
        "guard must not delete the ignored cache artifacts"
    );
}

#[test]
fn ignored_present_cache_artifacts_do_not_block_pre_publish() {
    let tmp = repo_with_ignored_present_cache(&[".pytest_cache/", "node_modules/"]);

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PrePublish),
    )
    .expect("scan artifacts");

    assert!(
        report.is_clean(),
        "gitignored+present cache artifacts can never be published and must not \
         block pre-publish (issue #928); got {:#?}",
        report.violations
    );
}

#[test]
fn worktree_mode_still_flags_ignored_present_cache_artifacts() {
    // Regression guard: the #928 narrowing must be scoped to pre-commit/pre-publish
    // only. The full-worktree audit modes must still surface ignored-present cache.
    let tmp = repo_with_ignored_present_cache(&[".pytest_cache/", "node_modules/"]);

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::Worktree),
    )
    .expect("scan artifacts");

    assert!(
        report
            .violations
            .iter()
            .any(|v| v.source == ArtifactSource::IgnoredPresent
                && v.path.starts_with("node_modules/")),
        "worktree mode must still flag ignored-present cache artifacts; got {:#?}",
        report.violations
    );
}

#[test]
fn all_mode_still_flags_ignored_present_cache_artifacts() {
    let tmp = repo_with_ignored_present_cache(&[".pytest_cache/", "node_modules/"]);

    let report =
        scan_artifacts(&ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::All))
            .expect("scan artifacts");

    assert!(
        report
            .violations
            .iter()
            .any(|v| v.source == ArtifactSource::IgnoredPresent
                && v.path.starts_with("node_modules/")),
        "all mode must still flag ignored-present cache artifacts; got {:#?}",
        report.violations
    );
}

#[test]
fn staged_committable_artifact_still_blocks_under_pre_commit_and_pre_publish() {
    // Regression guard: the #928 fix must only drop ignored-present blocking. A
    // *staged* prohibited artifact could actually be committed/published and must
    // still fail closed under both narrowed modes.
    for mode in [ArtifactGuardMode::PreCommit, ArtifactGuardMode::PrePublish] {
        let tmp = repo();
        write_file(
            &tmp.path().join("node_modules/leak/index.js"),
            "module.exports = 1;\n",
        );
        run_git(tmp.path(), &["add", "-f", "node_modules/leak/index.js"]);

        let report = scan_artifacts(&ArtifactGuardConfig::new(tmp.path()).with_mode(mode))
            .expect("scan artifacts");

        assert!(
            report.has_violations(),
            "staged committable artifact must still block under {mode}; got clean report"
        );
        violation_for(
            &report.violations,
            "node_modules/leak/index.js",
            ArtifactSource::Staged,
        );
    }
}

#[test]
fn tracked_committable_artifact_still_blocks_under_pre_publish() {
    let tmp = repo();
    write_file(
        &tmp.path().join("dist/plugin.js"),
        "committed generated bundle\n",
    );
    run_git(tmp.path(), &["add", "-f", "dist/plugin.js"]);
    run_git(tmp.path(), &["commit", "-qm", "accidentally commit bundle"]);

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PrePublish),
    )
    .expect("scan artifacts");

    violation_for(
        &report.violations,
        "dist/plugin.js",
        ArtifactSource::Tracked,
    );
}

#[test]
fn untracked_but_not_ignored_artifact_still_blocks_under_pre_commit() {
    // A cache-looking artifact that is present and untracked but NOT gitignored
    // *could* still be `git add`-ed and committed, so it must keep failing the
    // narrowed modes. Only the ignored-present source is exempted by #928.
    let tmp = repo();
    write_file(&tmp.path().join("dist/plugin.js"), "generated bundle\n");

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PreCommit),
    )
    .expect("scan artifacts");

    violation_for(
        &report.violations,
        "dist/plugin.js",
        ArtifactSource::Untracked,
    );
}

// ---------------------------------------------------------------------------
// Issue #1422: provenance — artifacts THIS change created vs a pre-existing
// repository condition.
//
// A `smart-orchestrator` run designed, tested, implemented, and verified its
// work over ninety minutes, then died at `checkpoint-after-implementation`
// because the repository it was working in had 668 `node_modules` paths
// committed long before the run started. The finding was true; the run did not
// cause it; none of the printed remedies fit inside the change's scope. The
// guard must separate the two cases and answer each on its own terms.
// ---------------------------------------------------------------------------

/// A repository shaped like every workflow run: a `main` base branch plus a
/// task branch holding the change under review.
fn repo_on_main() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    run_git(tmp.path(), &["init", "-q"]);
    // Pin the base branch name so baseline resolution does not depend on the
    // host's `init.defaultBranch`.
    run_git(tmp.path(), &["symbolic-ref", "HEAD", "refs/heads/main"]);
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

/// `main` already carries committed `node_modules`; the task branch makes an
/// unrelated source change. This is the issue #1422 repository exactly.
fn repo_with_preexisting_tracked_node_modules() -> TempDir {
    let tmp = repo_on_main();
    write_file(
        &tmp.path().join("node_modules/leftpad/index.js"),
        "module.exports = 1;\n",
    );
    write_file(&tmp.path().join("node_modules/.bin/acorn"), "#!/bin/sh\n");
    run_git(tmp.path(), &["add", "-f", "node_modules"]);
    run_git(
        tmp.path(),
        &["commit", "-qm", "history: vendored node_modules"],
    );
    run_git(tmp.path(), &["checkout", "-q", "-b", "task/pin-deps"]);
    write_file(&tmp.path().join("package.json"), "{\"name\":\"pinned\"}\n");
    run_git(tmp.path(), &["add", "package.json"]);
    run_git(tmp.path(), &["commit", "-qm", "pin dependency versions"]);
    tmp
}

#[test]
fn preexisting_tracked_artifacts_are_reported_but_do_not_block_the_publish_gate() {
    let tmp = repo_with_preexisting_tracked_node_modules();

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PrePublish),
    )
    .expect("scan artifacts");

    let violation = violation_for(
        &report.violations,
        "node_modules/leftpad/index.js",
        ArtifactSource::Tracked,
    );
    assert_eq!(
        violation.provenance,
        ArtifactProvenance::PreExisting,
        "a path committed on the baseline and untouched by this change is pre-existing"
    );
    assert!(
        report.baseline.is_some(),
        "the baseline must resolve so the report can say what provenance was measured against"
    );
    assert!(
        !report.blocks(),
        "a pre-existing repository condition must not fail the publish gate (#1422); blocking: {:#?}",
        report.blocking_violations()
    );
    assert_eq!(
        report.advisory_violations().len(),
        report.violations.len(),
        "every pre-existing violation must still be reported, just not blocking"
    );
}

#[test]
fn artifacts_introduced_by_this_change_still_block_the_publish_gate() {
    let tmp = repo_on_main();
    run_git(tmp.path(), &["checkout", "-q", "-b", "task/pin-deps"]);
    write_file(
        &tmp.path().join("node_modules/leftpad/index.js"),
        "module.exports = 1;\n",
    );
    run_git(tmp.path(), &["add", "-f", "node_modules"]);
    run_git(tmp.path(), &["commit", "-qm", "oops: commit node_modules"]);

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PrePublish),
    )
    .expect("scan artifacts");

    let violation = violation_for(
        &report.violations,
        "node_modules/leftpad/index.js",
        ArtifactSource::Tracked,
    );
    assert_eq!(
        violation.provenance,
        ArtifactProvenance::Introduced,
        "a path this branch committed since the baseline was introduced by this change"
    );
    assert!(
        report.blocks(),
        "artifacts this change introduced must still fail closed"
    );
}

#[test]
fn preexisting_policy_block_restores_the_fail_closed_gate() {
    let tmp = repo_with_preexisting_tracked_node_modules();

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path())
            .with_mode(ArtifactGuardMode::PrePublish)
            .with_preexisting_policy(PreExistingPolicy::Block),
    )
    .expect("scan artifacts");

    assert!(
        report.blocks(),
        "--preexisting block must restore whole-repository enforcement"
    );
    assert!(
        report.advisory_violations().is_empty(),
        "nothing is advisory under the block policy"
    );
}

#[test]
fn audit_modes_stay_fail_closed_on_a_preexisting_condition() {
    let tmp = repo_with_preexisting_tracked_node_modules();

    for mode in [ArtifactGuardMode::All, ArtifactGuardMode::Worktree] {
        let report = scan_artifacts(&ArtifactGuardConfig::new(tmp.path()).with_mode(mode))
            .expect("scan artifacts");

        assert!(
            report.blocks(),
            "{mode} is an audit scan, not a gate: it must keep failing closed so the \
             condition still has a place that reports it in full"
        );
        assert!(
            report.advisory_violations().is_empty(),
            "{mode} must not soften anything"
        );
    }
}

#[test]
fn untracked_artifacts_are_introduced_because_broad_staging_would_sweep_them_in() {
    // A leftover on disk is not "the repository's history": the very next
    // `git add -A` puts it in the commit. It must stay blocking.
    let tmp = repo_on_main();
    run_git(tmp.path(), &["checkout", "-q", "-b", "task/pin-deps"]);
    write_file(&tmp.path().join("dist/plugin.js"), "generated bundle\n");

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PrePublish),
    )
    .expect("scan artifacts");

    let violation = violation_for(
        &report.violations,
        "dist/plugin.js",
        ArtifactSource::Untracked,
    );
    assert_eq!(violation.provenance, ArtifactProvenance::Introduced);
    assert!(
        report.blocks(),
        "untracked artifacts must still fail closed"
    );
}

#[test]
fn a_tracked_artifact_this_change_modified_is_introduced_not_preexisting() {
    let tmp = repo_with_preexisting_tracked_node_modules();
    write_file(
        &tmp.path().join("node_modules/leftpad/index.js"),
        "module.exports = 2;\n",
    );
    run_git(tmp.path(), &["add", "-f", "node_modules/leftpad/index.js"]);
    run_git(tmp.path(), &["commit", "-qm", "edit vendored artifact"]);

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PrePublish),
    )
    .expect("scan artifacts");

    let violation = violation_for(
        &report.violations,
        "node_modules/leftpad/index.js",
        ArtifactSource::Tracked,
    );
    assert_eq!(
        violation.provenance,
        ArtifactProvenance::Introduced,
        "touching a pre-existing artifact pulls it into this change's diff"
    );
    assert!(report.blocks());
}

#[test]
fn an_explicit_baseline_selects_what_counts_as_this_change() {
    let tmp = repo_with_preexisting_tracked_node_modules();

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path())
            .with_mode(ArtifactGuardMode::PrePublish)
            .with_baseline("main"),
    )
    .expect("scan artifacts");

    assert_eq!(
        report
            .baseline
            .as_ref()
            .map(|baseline| baseline.rev.clone()),
        Some("main".to_string())
    );
    assert!(!report.blocks());
}

#[test]
fn an_unresolvable_explicit_baseline_fails_closed_as_configuration_error() {
    let tmp = repo_with_preexisting_tracked_node_modules();

    let error = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path())
            .with_mode(ArtifactGuardMode::PrePublish)
            .with_baseline("refs/heads/does-not-exist"),
    )
    .expect_err("an explicitly requested baseline that cannot resolve is a misconfiguration");

    assert!(
        matches!(error, super::ArtifactGuardError::Baseline { .. }),
        "expected a baseline error; got {error:?}"
    );
}
