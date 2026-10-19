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
    // `rm -rf -- 'worktrees'` would delete the registered sibling too.
    for violation in &report.violations {
        assert_ne!(
            violation.removal_root.as_deref(),
            Some("worktrees"),
            "a root holding a registered worktree must be narrowed: {violation:?}"
        );
    }
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

// ---------------------------------------------------------------------------
// Issue #1536: the generic output-segment rule (`dist`, `build`, `coverage`,
// `out`, `logs`, `outputs`) flagged real source -- a tracked
// `apps/coverage/tsconfig.json` -- as a build artifact, the report then told
// the operator to untrack it, and "introduced" was measured against
// `@{upstream}`, so a base-branch merge was charged to the PR.
//
// Contracts:
// - A segment directory `D` is exempt when the index holds `D/<manifest>` as a
//   direct child. Root, sibling and parent manifests never exempt; each
//   matching segment is judged on its own.
// - Every violation carries `removal_root`: the narrowest path that is wholly
//   generated, or `None` when the guard must never suggest removing it.
// - The default baseline prefers the base branch over `@{upstream}`.
// ---------------------------------------------------------------------------

use super::{BASELINE_CANDIDATES, manifest_dirs, read_index, rule_for_path};
use std::collections::BTreeSet;

fn manifest_set(dirs: &[&str]) -> BTreeSet<String> {
    dirs.iter().map(|dir| (*dir).to_string()).collect()
}

/// A `Some` removal root must never reach outside the matched path: non-empty,
/// repo-relative, and equal to the path or a `/`-bounded prefix of it.
fn assert_removal_root_invariants(path: &str, root: &str) {
    assert!(!root.is_empty(), "{path}: removal root must not be empty");
    assert!(
        !root.starts_with('/'),
        "{path}: removal root {root:?} must be repo-relative"
    );
    assert!(
        !root.split('/').any(|part| part == ".." || part.is_empty()),
        "{path}: removal root {root:?} must not traverse or hold empty segments"
    );
    assert!(
        path == root || path.starts_with(&format!("{root}/")),
        "{path}: removal root {root:?} must be the path or a /-bounded prefix of it"
    );
}

/// (path, source, indexed manifest dirs, expected (rule id, removal root)).
type RuleCase = (
    &'static str,
    ArtifactSource,
    &'static [&'static str],
    Option<(&'static str, Option<&'static str>)>,
);

#[test]
fn rule_table_with_a_manifest_set() {
    use ArtifactSource::{IgnoredPresent, Staged, Tracked, Untracked};

    // (path, source, indexed manifest dirs, expected (rule, removal_root)).
    let cases: &[RuleCase] = &[
        // Bug 1: a directory holding its own manifest is source, not output.
        (
            "apps/coverage/tsconfig.json",
            Tracked,
            &["apps/coverage"],
            None,
        ),
        (
            "apps/coverage/package.json",
            Staged,
            &["apps/coverage"],
            None,
        ),
        (
            "apps/coverage/src/index.ts",
            Tracked,
            &["apps/coverage"],
            None,
        ),
        (
            "apps/coverage/src/index.ts",
            Untracked,
            &["apps/coverage"],
            None,
        ),
        ("tools/build/Cargo.toml", Tracked, &["tools/build"], None),
        ("svc/out/go.mod", Tracked, &["svc/out"], None),
        ("py/logs/pyproject.toml", Tracked, &["py/logs"], None),
        // Coverage reports are only matched inside a non-exempt segment.
        ("apps/coverage/lcov.info", Tracked, &["apps/coverage"], None),
        ("lcov.info", Tracked, &[], None),
        // Documented trade-off: a tracked manifest inside `dist/` exempts it.
        (
            "packages/x/dist/index.js",
            Tracked,
            &["packages/x/dist"],
            None,
        ),
        // Each matching segment is checked on its own.
        (
            "apps/coverage/dist/x.js",
            Untracked,
            &["apps/coverage"],
            Some(("build-artifact", Some("apps/coverage/dist"))),
        ),
        (
            "apps/coverage/dist/x.js",
            Tracked,
            &["apps/coverage"],
            Some(("build-artifact", None)),
        ),
        (
            "out/coverage/x.txt",
            Untracked,
            &["out/coverage"],
            Some(("build-artifact", Some("out"))),
        ),
        // Sibling / parent manifests never exempt the output directory.
        (
            "pkg/dist/index.js",
            Untracked,
            &["pkg"],
            Some(("build-artifact", Some("pkg/dist"))),
        ),
        ("dist/app.js", Tracked, &[], Some(("build-artifact", None))),
        // Unambiguously generated coverage reports keep an exact removal root.
        (
            "coverage/lcov.info",
            Tracked,
            &[],
            Some(("build-artifact", Some("coverage/lcov.info"))),
        ),
        (
            "coverage/lcov.info",
            Staged,
            &[],
            Some(("build-artifact", Some("coverage/lcov.info"))),
        ),
        (
            "web/coverage/coverage-final.json",
            Tracked,
            &["web"],
            Some(("build-artifact", Some("web/coverage/coverage-final.json"))),
        ),
        (
            "coverage/clover.xml",
            Staged,
            &[],
            Some(("build-artifact", Some("coverage/clover.xml"))),
        ),
        // A tracked or staged report outside a project's `coverage/` directory
        // may be a parser fixture: possibly source, no removal root.
        (
            "tests/fixtures/coverage/lcov.info",
            Tracked,
            &[],
            Some(("build-artifact", None)),
        ),
        (
            "tests/fixtures/coverage/lcov.info",
            Staged,
            &[],
            Some(("build-artifact", None)),
        ),
        (
            "coverage/fixtures/lcov.info",
            Tracked,
            &[],
            Some(("build-artifact", None)),
        ),
        // The same file untracked or ignored is a run's output.
        (
            "tests/fixtures/coverage/lcov.info",
            Untracked,
            &[],
            Some(("build-artifact", Some("tests/fixtures/coverage/lcov.info"))),
        ),
        (
            "tests/fixtures/coverage/lcov.info",
            IgnoredPresent,
            &[],
            Some(("build-artifact", Some("tests/fixtures/coverage/lcov.info"))),
        ),
        // Bug 2: tracked/staged generic-segment matches may be source, so they
        // get no removal root; untracked output may still be removed.
        (
            "build/notes.md",
            Tracked,
            &[],
            Some(("build-artifact", None)),
        ),
        ("logs/keep.md", Staged, &[], Some(("build-artifact", None))),
        (
            "build/out.o",
            Untracked,
            &[],
            Some(("build-artifact", Some("build"))),
        ),
        (
            "outputs/run/1.json",
            IgnoredPresent,
            &[],
            Some(("build-artifact", Some("outputs"))),
        ),
        // `target`, `.next` and `index.scip` are unambiguous and ignore manifests.
        (
            "target/debug/app",
            Untracked,
            &[],
            Some(("build-artifact", Some("target"))),
        ),
        (
            "target/debug/app",
            Tracked,
            &[],
            Some(("build-artifact", Some("target"))),
        ),
        ("target/debug/app", IgnoredPresent, &[], None),
        (
            "crates/foo/target/debug/app",
            Untracked,
            &["crates/foo"],
            Some(("build-artifact", Some("crates/foo/target"))),
        ),
        (
            "apps/coverage/target/x",
            Untracked,
            &["apps/coverage"],
            Some(("build-artifact", Some("apps/coverage/target"))),
        ),
        (
            "index.scip",
            Untracked,
            &[],
            Some(("build-artifact", Some("index.scip"))),
        ),
        (
            ".next/server/page.js",
            Untracked,
            &[],
            Some(("build-artifact", Some(".next"))),
        ),
        (
            "apps/web/.next/server/page.js",
            Tracked,
            &["apps/web"],
            Some(("build-artifact", Some("apps/web/.next"))),
        ),
        // Other rules: matching unchanged; root comes from the matched segment,
        // never from a shallower artifact-shaped ancestor.
        (
            "dist/plugin.js",
            Tracked,
            &[],
            Some(("plugin-bundle", Some("dist"))),
        ),
        (
            "node_modules/a/index.js",
            Tracked,
            &[],
            Some(("node-modules", Some("node_modules"))),
        ),
        (
            "apps/coverage/node_modules/a/index.js",
            Staged,
            &["apps/coverage"],
            Some(("node-modules", Some("apps/coverage/node_modules"))),
        ),
        (
            ".claude/runtime/x.json",
            Tracked,
            &[],
            Some(("claude-runtime", Some(".claude/runtime"))),
        ),
        (
            "worktrees/a/b.txt",
            Untracked,
            &[],
            Some(("nested-worktree", Some("worktrees"))),
        ),
        (
            "recipe-runner.log",
            Untracked,
            &[],
            Some(("workflow-session-artifact", Some("recipe-runner.log"))),
        ),
        (
            ".copilot/session-state/a.json",
            Untracked,
            &[],
            Some(("workflow-session-artifact", Some(".copilot/session-state"))),
        ),
        (
            "pkg/.pytest_cache/v/lastfailed",
            Untracked,
            &["pkg"],
            Some(("cache-artifact", Some("pkg/.pytest_cache"))),
        ),
        (
            ".yarn/cache/x.zip",
            Untracked,
            &[],
            Some(("cache-artifact", Some(".yarn/cache/x.zip"))),
        ),
        // Zero-install repos commit `.yarn/cache`; a root over it would sweep
        // archives the run never listed.
        (
            "pkg/.yarn/cache/new.zip",
            Staged,
            &[],
            Some(("cache-artifact", Some("pkg/.yarn/cache/new.zip"))),
        ),
        (
            "apps/web/.next/cache/webpack/x.pack",
            IgnoredPresent,
            &["apps/web"],
            Some(("cache-artifact", Some("apps/web/.next/cache"))),
        ),
        // Name look-alikes are not segments.
        ("dist2/app.js", Untracked, &[], None),
        ("buildtools/a.rs", Tracked, &[], None),
        ("README.md", Tracked, &[], None),
    ];

    for (path, source, dirs, expected) in cases {
        let manifests = manifest_set(dirs);
        let actual =
            rule_for_path(path, *source, &manifests).map(|(rule, root)| (rule.id, root.clone()));
        let expected = expected.map(|(id, root)| (id, root.map(str::to_string)));
        assert_eq!(
            actual, expected,
            "rule_for_path({path:?}, {source:?}, {dirs:?})"
        );
        if let Some((_, Some(root))) = &actual {
            assert_removal_root_invariants(path, root);
        }
    }
}

#[test]
fn a_repo_root_manifest_never_exempts_an_output_directory() {
    // The repo root is never a manifest dir, and even a literal "" entry must
    // not exempt top-level output.
    let manifests = manifest_set(&[""]);
    let matched = rule_for_path("dist/app.js", ArtifactSource::Untracked, &manifests)
        .map(|(rule, root)| (rule.id, root.clone()));
    assert_eq!(matched, Some(("build-artifact", Some("dist".to_string()))));
}

#[test]
fn manifest_dirs_lists_parents_of_indexed_manifests_only() {
    let tmp = repo();
    for path in [
        "package.json",
        "apps/coverage/package.json",
        "apps/coverage/tsconfig.json",
        "nested/deep/Cargo.toml",
        "py/pyproject.toml",
        "notmanifest/package.json.bak",
    ] {
        write_file(&tmp.path().join(path), "{}\n");
    }
    run_git(tmp.path(), &["add", "."]);
    run_git(tmp.path(), &["commit", "-qm", "manifests"]);
    // Staged but uncommitted: the index already holds it.
    write_file(&tmp.path().join("svc/go.mod"), "module svc\n");
    run_git(tmp.path(), &["add", "svc/go.mod"]);
    // Untracked: not in the index, so it must not count.
    write_file(&tmp.path().join("loose/package.json"), "{}\n");

    let dirs = manifest_dirs(&read_index(tmp.path()).expect("index"));

    assert_eq!(
        dirs,
        manifest_set(&["apps/coverage", "nested/deep", "py", "svc"]),
        "only indexed manifests count, by their parent directory, never the root"
    );
    assert!(!dirs.contains(""), "the repo root is never a manifest dir");
}

#[test]
fn read_index_fails_closed_outside_a_git_repository() {
    let tmp = TempDir::new().expect("tempdir");
    let error = read_index(tmp.path())
        .expect_err("a failed git ls-files must not read as an empty, exempt set");
    assert!(
        matches!(error, super::ArtifactGuardError::Git { .. }),
        "expected a git error; got {error:?}"
    );
}

#[test]
fn tracked_app_named_coverage_is_source_not_a_build_artifact() {
    let tmp = repo_on_main();
    run_git(tmp.path(), &["checkout", "-q", "-b", "task/app"]);
    write_file(
        &tmp.path().join("apps/coverage/package.json"),
        "{\"name\":\"coverage\"}\n",
    );
    write_file(&tmp.path().join("apps/coverage/tsconfig.json"), "{}\n");
    write_file(
        &tmp.path().join("apps/coverage/src/index.ts"),
        "export const x = 1;\n",
    );
    run_git(tmp.path(), &["add", "apps"]);

    // Staged first (pre-commit sees the staged scan), then committed.
    for mode in [ArtifactGuardMode::PreCommit, ArtifactGuardMode::PrePublish] {
        let report = scan_artifacts(&ArtifactGuardConfig::new(tmp.path()).with_mode(mode))
            .expect("scan artifacts");
        assert!(
            report.is_clean(),
            "staged apps/coverage source must not be flagged in {mode}; got {:#?}",
            report.violations
        );
    }
    run_git(tmp.path(), &["commit", "-qm", "add the coverage app"]);
    for mode in [
        ArtifactGuardMode::PreCommit,
        ArtifactGuardMode::PrePublish,
        ArtifactGuardMode::All,
    ] {
        let report = scan_artifacts(&ArtifactGuardConfig::new(tmp.path()).with_mode(mode))
            .expect("scan artifacts");
        assert!(
            report.is_clean(),
            "tracked apps/coverage source must not be flagged in {mode}; got {:#?}",
            report.violations
        );
    }
}

#[test]
fn coverage_reports_and_dist_bundles_are_still_caught() {
    let tmp = repo_on_main();
    run_git(tmp.path(), &["checkout", "-q", "-b", "task/leak"]);
    write_file(&tmp.path().join("package.json"), "{\"name\":\"root\"}\n");
    write_file(&tmp.path().join("coverage/lcov.info"), "TN:\n");
    write_file(&tmp.path().join("dist/app.js"), "bundle\n");
    write_file(&tmp.path().join("dist/plugin.js"), "bundle\n");
    run_git(tmp.path(), &["add", "-f", "."]);
    run_git(tmp.path(), &["commit", "-qm", "oops: commit outputs"]);
    write_file(&tmp.path().join("coverage/lcov.info"), "TN:\nSF:a\n");
    run_git(tmp.path(), &["add", "-f", "coverage/lcov.info"]);

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PrePublish),
    )
    .expect("scan artifacts");

    for (path, source, rule_id, root) in [
        (
            "coverage/lcov.info",
            ArtifactSource::Staged,
            "build-artifact",
            Some("coverage/lcov.info"),
        ),
        (
            "coverage/lcov.info",
            ArtifactSource::Tracked,
            "build-artifact",
            Some("coverage/lcov.info"),
        ),
        (
            "dist/app.js",
            ArtifactSource::Tracked,
            "build-artifact",
            None,
        ),
        // `dist/` also holds `dist/app.js`, which may be source, so the bundle
        // is narrowed to itself rather than taking `dist` along with it.
        (
            "dist/plugin.js",
            ArtifactSource::Tracked,
            "plugin-bundle",
            Some("dist/plugin.js"),
        ),
    ] {
        let violation = violation_for(&report.violations, path, source);
        assert_eq!(violation.rule_id, rule_id, "{path} ({source})");
        assert_eq!(
            violation.removal_root.as_deref(),
            root,
            "{path} ({source}) removal root"
        );
        assert_eq!(violation.provenance, ArtifactProvenance::Introduced);
    }
    assert!(report.blocks(), "introduced outputs must still fail closed");
}

#[test]
fn untracked_files_in_a_source_directory_are_not_flagged_but_output_beneath_it_is() {
    let tmp = repo();
    write_file(&tmp.path().join("apps/coverage/package.json"), "{}\n");
    run_git(tmp.path(), &["add", "apps/coverage/package.json"]);
    run_git(tmp.path(), &["commit", "-qm", "coverage app"]);
    write_file(&tmp.path().join("apps/coverage/src/new.ts"), "export {};\n");
    write_file(&tmp.path().join("apps/coverage/dist/x.js"), "bundle\n");

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.path == "apps/coverage/src/new.ts"),
        "new source in an evidently-source directory must pass; got {:#?}",
        report.violations
    );
    let violation = violation_for(
        &report.violations,
        "apps/coverage/dist/x.js",
        ArtifactSource::Untracked,
    );
    assert_eq!(violation.rule_id, "build-artifact");
    assert_eq!(
        violation.removal_root.as_deref(),
        Some("apps/coverage/dist")
    );
}

#[test]
fn an_untracked_manifest_does_not_exempt_its_directory() {
    let tmp = repo();
    write_file(&tmp.path().join("build/package.json"), "{}\n");
    write_file(&tmp.path().join("build/bundle.js"), "bundle\n");

    let report = scan_artifacts(&default_config(tmp.path())).expect("scan artifacts");

    let violation = violation_for(
        &report.violations,
        "build/bundle.js",
        ArtifactSource::Untracked,
    );
    assert_eq!(violation.rule_id, "build-artifact");
}

#[test]
fn a_tracked_ambiguous_output_path_is_reported_without_a_removal_root() {
    let tmp = repo_on_main();
    run_git(tmp.path(), &["checkout", "-q", "-b", "task/notes"]);
    write_file(&tmp.path().join("build/notes.md"), "# notes\n");
    run_git(tmp.path(), &["add", "-f", "build/notes.md"]);
    run_git(tmp.path(), &["commit", "-qm", "notes"]);

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::PrePublish),
    )
    .expect("scan artifacts");

    let violation = violation_for(
        &report.violations,
        "build/notes.md",
        ArtifactSource::Tracked,
    );
    assert_eq!(violation.rule_id, "build-artifact");
    assert_eq!(
        violation.removal_root, None,
        "a tracked generic-segment match may be source: never suggest removing it"
    );
    assert!(report.blocks());
}

#[test]
fn default_baseline_prefers_the_base_branch_over_upstream() {
    assert_eq!(
        BASELINE_CANDIDATES,
        &[
            "origin/HEAD",
            "origin/main",
            "origin/master",
            "main",
            "master",
            "@{upstream}",
        ],
        "@{{upstream}} is the PR branch's own remote copy: measuring against it \
         charges every base-branch merge to the PR (#1536), so it is the last resort"
    );
}

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let output = amplihack_git::command()
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?}: {e}"));
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn a_base_branch_merge_is_not_attributed_to_the_pr() {
    let root = TempDir::new().expect("tempdir");
    let origin = root.path().join("origin.git");
    let work = root.path().join("work");
    run_git(root.path(), &["init", "-q", "--bare", "origin.git"]);
    run_git(&origin, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    run_git(root.path(), &["clone", "-q", "origin.git", "work"]);
    run_git(&work, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    run_git(
        &work,
        &["config", "user.email", "artifact-guard@example.invalid"],
    );
    run_git(&work, &["config", "user.name", "Artifact Guard Test"]);
    write_file(&work.join("README.md"), "# fixture\n");
    run_git(&work, &["add", "README.md"]);
    run_git(&work, &["commit", "-qm", "initial"]);
    run_git(&work, &["push", "-q", "-u", "origin", "main"]);

    // The PR branch, pushed so `@{upstream}` exists.
    run_git(&work, &["checkout", "-q", "-b", "feature"]);
    write_file(&work.join("src/lib.rs"), "pub fn f() {}\n");
    run_git(&work, &["add", "src/lib.rs"]);
    run_git(&work, &["commit", "-qm", "feature work"]);
    run_git(&work, &["push", "-q", "-u", "origin", "feature"]);
    let pre_merge_head = git_stdout(&work, &["rev-parse", "HEAD"]);

    // Someone lands an artifact-shaped path on main.
    run_git(&work, &["checkout", "-q", "main"]);
    write_file(&work.join("build/out.log"), "log\n");
    run_git(&work, &["add", "-f", "build/out.log"]);
    run_git(&work, &["commit", "-qm", "main: track a build log"]);
    run_git(&work, &["push", "-q", "origin", "main"]);

    // The PR merges its base branch to stay current; not pushed yet.
    run_git(&work, &["checkout", "-q", "feature"]);
    run_git(&work, &["fetch", "-q", "origin"]);
    run_git(
        &work,
        &["merge", "-q", "--no-ff", "-m", "merge main", "origin/main"],
    );
    run_git(&work, &["remote", "set-head", "origin", "main"]);
    assert_eq!(
        git_stdout(&work, &["rev-parse", "@{upstream}"]),
        pre_merge_head,
        "fixture: @{{upstream}} still points at the pre-merge head"
    );

    let report =
        scan_artifacts(&ArtifactGuardConfig::new(&work).with_mode(ArtifactGuardMode::PrePublish))
            .expect("scan artifacts");

    let baseline = report.baseline.as_ref().expect("a baseline must resolve");
    assert_eq!(baseline.rev, "origin/HEAD");
    assert_eq!(
        baseline.commit,
        git_stdout(&work, &["rev-parse", "origin/main"]),
        "the merge base with the base branch is its tip after the merge"
    );
    let violation = violation_for(&report.violations, "build/out.log", ArtifactSource::Tracked);
    assert_eq!(
        violation.provenance,
        ArtifactProvenance::PreExisting,
        "a path the base branch brought in through a merge is not the PR's change"
    );
    assert!(
        !report.blocks(),
        "the base-branch merge must not fail the PR's publish gate; blocking: {:#?}",
        report.blocking_violations()
    );
}

#[test]
fn is_valid_allowlist_entry_matches_the_parser_rules() {
    for refused in ["build", "node_modules", "logs", "/abs", "*"] {
        assert!(
            !super::is_valid_allowlist_entry(refused),
            "{refused:?} must be refused, as the allowlist parser refuses it"
        );
    }
    for accepted in ["docs/build", "src/logs.rs", "pkg/node_modules/x.js"] {
        assert!(
            super::is_valid_allowlist_entry(accepted),
            "{accepted:?} must be accepted, as the allowlist parser accepts it"
        );
    }
}

#[test]
fn a_directory_holding_allowlisted_source_is_never_a_removal_root() {
    // Allowlisted `docs/build/guide.md` never becomes a violation, so the guard
    // must learn from the index that `docs/build` holds source; otherwise the
    // untracked leftover beside it gets `rm -rf -- 'docs/build'` (issue #1536).
    let tmp = repo_on_main();
    run_git(tmp.path(), &["checkout", "-q", "-b", "task/docs"]);
    write_file(&tmp.path().join("docs/build/guide.md"), "# guide\n");
    write_file(
        &tmp.path().join(".amplihack-artifact-allowlist"),
        "docs/build/guide.md\n",
    );
    run_git(tmp.path(), &["add", "-f", "."]);
    run_git(tmp.path(), &["commit", "-qm", "docs"]);
    write_file(&tmp.path().join("docs/build/scratch.log"), "x\n");
    write_file(&tmp.path().join("other/build/x.o"), "obj\n");

    let config = ArtifactGuardConfig::new(tmp.path())
        .with_mode(ArtifactGuardMode::PreCommit)
        .with_allowlist(tmp.path().join(".amplihack-artifact-allowlist"));
    let report = scan_artifacts(&config).expect("scan artifacts");

    let root_of = |path: &str| {
        report
            .violations
            .iter()
            .find(|violation| violation.path == path)
            .unwrap_or_else(|| panic!("{path} must be reported; got {:#?}", report.violations))
            .removal_root
            .clone()
    };
    assert_eq!(
        root_of("docs/build/scratch.log"),
        Some("docs/build/scratch.log".to_string()),
        "a root holding allowlisted source must narrow to the exact path"
    );
    assert_eq!(
        root_of("other/build/x.o"),
        Some("other/build".to_string()),
        "a root holding nothing indexed keeps its whole-directory remedy"
    );
    assert!(
        report
            .violations
            .iter()
            .all(|violation| violation.path != "docs/build/guide.md"),
        "the allowlisted file must not be reported"
    );
}

#[test]
fn an_untracked_allowlisted_file_is_never_swept_into_a_removal_root() {
    // The allowlisted file is in neither the index nor the violations, so only
    // the untracked scan itself can protect it.
    let tmp = repo_on_main();
    write_file(
        &tmp.path().join(".amplihack-artifact-allowlist"),
        "docs/build/guide.md\n",
    );
    write_file(&tmp.path().join("docs/build/guide.md"), "# guide\n");
    write_file(&tmp.path().join("docs/build/scratch.log"), "x\n");

    let config = ArtifactGuardConfig::new(tmp.path())
        .with_mode(ArtifactGuardMode::Worktree)
        .with_allowlist(tmp.path().join(".amplihack-artifact-allowlist"));
    let report = scan_artifacts(&config).expect("scan artifacts");

    let scratch = violation_for(
        &report.violations,
        "docs/build/scratch.log",
        ArtifactSource::Untracked,
    );
    assert_eq!(
        scratch.removal_root.as_deref(),
        Some("docs/build/scratch.log"),
        "the root must not take the allowlisted file with it"
    );
}

#[test]
fn removal_root_check_rejects_anything_but_the_path_or_a_bounded_prefix() {
    // Enforced in release builds: a root failing it becomes None (manual
    // review), never a removal command.
    use super::is_removal_root_of;
    assert!(is_removal_root_of("node_modules", "node_modules/a/b.js"));
    assert!(is_removal_root_of(
        "coverage/lcov.info",
        "coverage/lcov.info"
    ));
    for (root, path) in [
        ("", "a/b"),
        ("/node_modules", "/node_modules/a"),
        ("a/../b", "a/../b/c"),
        ("a//b", "a//b/c"),
        ("build", "buildtools/a.rs"),
        ("other", "node_modules/a.js"),
    ] {
        assert!(!is_removal_root_of(root, path), "{root:?} for {path:?}");
    }
}

/// A stacked PR (cut from another feature branch) measured against the default
/// branch has its parent's paths attributed to it. That is the documented
/// limitation of the default baseline; `--baseline origin/<base>` is the remedy
/// (`docs/artifact-guard.md`, "Stacked PRs").
#[test]
fn a_stacked_pr_needs_its_base_branch_as_an_explicit_baseline() {
    let root = TempDir::new().expect("tempdir");
    let origin = root.path().join("origin.git");
    let work = root.path().join("work");
    run_git(root.path(), &["init", "-q", "--bare", "origin.git"]);
    run_git(&origin, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    run_git(root.path(), &["clone", "-q", "origin.git", "work"]);
    run_git(&work, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    run_git(
        &work,
        &["config", "user.email", "artifact-guard@example.invalid"],
    );
    run_git(&work, &["config", "user.name", "Artifact Guard Test"]);
    write_file(&work.join("README.md"), "# fixture\n");
    run_git(&work, &["add", "README.md"]);
    run_git(&work, &["commit", "-qm", "initial"]);
    run_git(&work, &["push", "-q", "-u", "origin", "main"]);
    run_git(&work, &["remote", "set-head", "origin", "main"]);

    // The parent PR tracks an artifact-shaped path.
    run_git(&work, &["checkout", "-q", "-b", "parent"]);
    write_file(&work.join("build/out.log"), "log\n");
    run_git(&work, &["add", "-f", "build/out.log"]);
    run_git(&work, &["commit", "-qm", "parent: track a build log"]);
    run_git(&work, &["push", "-q", "-u", "origin", "parent"]);

    // The child PR is cut from the parent and touches only source.
    run_git(&work, &["checkout", "-q", "-b", "child"]);
    write_file(&work.join("src/lib.rs"), "pub fn f() {}\n");
    run_git(&work, &["add", "src/lib.rs"]);
    run_git(&work, &["commit", "-qm", "child work"]);

    let config = ArtifactGuardConfig::new(&work).with_mode(ArtifactGuardMode::PrePublish);
    let default = scan_artifacts(&config).expect("scan artifacts");
    assert_eq!(
        default.baseline.as_ref().map(|b| b.rev.as_str()),
        Some("origin/HEAD")
    );
    assert_eq!(
        violation_for(
            &default.violations,
            "build/out.log",
            ArtifactSource::Tracked
        )
        .provenance,
        ArtifactProvenance::Introduced,
        "the default baseline is the repository's default branch, so the parent's \
         path counts as introduced by the child"
    );
    assert!(
        default.blocks(),
        "the limitation blocks; it never lets a path through"
    );

    let explicit = scan_artifacts(&config.with_baseline("origin/parent")).expect("scan artifacts");
    assert_eq!(
        violation_for(
            &explicit.violations,
            "build/out.log",
            ArtifactSource::Tracked
        )
        .provenance,
        ArtifactProvenance::PreExisting,
        "measured against its own base branch, the child introduced nothing"
    );
    assert!(!explicit.blocks(), "{:#?}", explicit.blocking_violations());
}

/// `staged` mode reads only the indexed manifests, not the whole index, yet
/// the manifest exemption must still hold there.
#[test]
fn staged_mode_still_honours_an_indexed_manifest_exemption() {
    let tmp = repo();
    write_file(&tmp.path().join("apps/coverage/package.json"), "{}\n");
    run_git(tmp.path(), &["add", "apps/coverage/package.json"]);
    run_git(tmp.path(), &["commit", "-qm", "app"]);
    write_file(
        &tmp.path().join("apps/coverage/src/index.ts"),
        "export {}\n",
    );
    write_file(&tmp.path().join("dist/bundle.js"), "x\n");
    run_git(
        tmp.path(),
        &["add", "-f", "apps/coverage/src/index.ts", "dist/bundle.js"],
    );

    let report =
        scan_artifacts(&ArtifactGuardConfig::new(tmp.path()).with_mode(ArtifactGuardMode::Staged))
            .expect("scan artifacts");
    let paths: Vec<&str> = report.violations.iter().map(|v| v.path.as_str()).collect();
    assert_eq!(paths, ["dist/bundle.js"], "{:#?}", report.violations);
}

// A failing `git worktree list` ends the scan with an error in every mode
// (issue #1528), so #1536's fallback that narrowed every directory root when
// the list was unknown has nothing left to handle. The fail-loud contract is
// tested by `worktree_list_failure_exits_2_in_every_mode_and_never_prints_rm_rf`
// (tests/integration/artifact_guard_cli_tests.rs) and the `WorktreeList` tests
// below.

// ---------------------------------------------------------------------------
// Issue #1528: a run worktree recorded as a gitlink in the index.
//
// Auto-drive phases 2 and 3 committed in the launch checkout, and its index
// ended up holding the run worktrees under `worktrees/` as gitlinks (mode
// 160000). The `run-worktree-gitlink` rule refuses such an index in every
// mode. It reads the whole index, so a gitlink already in `HEAD` is caught too.
// The allowlist, the pre-existing policy and the baseline never soften it.
// ---------------------------------------------------------------------------

const GITLINK_RULE: &str = "run-worktree-gitlink";

const ALL_MODES: [ArtifactGuardMode; 5] = [
    ArtifactGuardMode::All,
    ArtifactGuardMode::Staged,
    ArtifactGuardMode::Worktree,
    ArtifactGuardMode::PreCommit,
    ArtifactGuardMode::PrePublish,
];

/// Run git with no global or system config, no prompts and a fixed identity,
/// so a developer's config (signing, hooks, `init.defaultBranch`) cannot change
/// a fixture. Returns stdout.
fn hgit(repo: &Path, args: &[&str]) -> String {
    let output = amplihack_git::command()
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Artifact Guard Test")
        .env("GIT_AUTHOR_EMAIL", "artifact-guard@example.invalid")
        .env("GIT_COMMITTER_NAME", "Artifact Guard Test")
        .env("GIT_COMMITTER_EMAIL", "artifact-guard@example.invalid")
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", repo.display()));
    assert!(
        output.status.success(),
        "git {args:?} failed in {}\nstdout:\n{}\nstderr:\n{}",
        repo.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Feed NUL-terminated `git update-index -z --index-info` records, which lets
/// a fixture write any index entry (any mode, any stage, any path bytes).
fn hgit_index_info(repo: &Path, records: &[(&str, &str, u8, &str)]) {
    use std::io::Write;
    use std::process::Stdio;

    let mut input = Vec::new();
    for (mode, sha, stage, path) in records {
        input.extend_from_slice(format!("{mode} {sha} {stage}\t{path}").as_bytes());
        input.push(0);
    }
    let mut child = amplihack_git::command()
        .args(["update-index", "-z", "--index-info"])
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn git update-index");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&input)
        .expect("write index-info");
    let output = child.wait_with_output().expect("wait git update-index");
    assert!(
        output.status.success(),
        "git update-index --index-info failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repository on `main` whose `.gitignore` excludes `worktrees/`, the shape of
/// the launch checkout in issue #1528.
fn gitlink_repo() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    hgit(tmp.path(), &["init", "-q"]);
    hgit(tmp.path(), &["symbolic-ref", "HEAD", "refs/heads/main"]);
    write_file(&tmp.path().join("README.md"), "# fixture\n");
    write_file(&tmp.path().join(".gitignore"), "worktrees/\n");
    hgit(tmp.path(), &["add", "README.md", ".gitignore"]);
    hgit(tmp.path(), &["commit", "-qm", "initial"]);
    tmp
}

fn head_sha(repo: &Path) -> String {
    hgit(repo, &["rev-parse", "HEAD"]).trim().to_string()
}

/// Register a linked worktree at repo-relative `path` on a new branch.
fn register_worktree(repo: &Path, path: &str, branch: &str) {
    hgit(repo, &["worktree", "add", "-q", path, "-b", branch]);
}

/// Repo-relative paths of every gitlink (mode 160000) in the index.
fn index_gitlinks(repo: &Path) -> Vec<String> {
    let mut paths: Vec<String> = hgit(repo, &["ls-files", "-s", "-z"])
        .split('\0')
        .filter_map(|record| {
            let (meta, path) = record.split_once('\t')?;
            meta.starts_with("160000 ").then(|| path.to_string())
        })
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Stage the worktree at `path` as a gitlink the way a forced add does, then
/// prove the fixture really produced a gitlink.
fn force_stage_gitlink(repo: &Path, path: &str) {
    hgit(repo, &["add", "-f", "--", path]);
    assert!(
        index_gitlinks(repo).iter().any(|p| p == path),
        "fixture must stage {path} as a gitlink; index gitlinks: {:?}",
        index_gitlinks(repo)
    );
}

/// The index as git reports it: every entry, and the staged diff. A scan that
/// is report-only leaves both byte-for-byte unchanged.
fn index_snapshot(repo: &Path) -> (String, String) {
    (
        hgit(repo, &["ls-files", "-s"]),
        hgit(repo, &["diff", "--cached", "--raw"]),
    )
}

fn scan(repo: &Path, mode: ArtifactGuardMode) -> super::ArtifactGuardReport {
    scan_artifacts(&ArtifactGuardConfig::new(repo).with_mode(mode))
        .unwrap_or_else(|e| panic!("scan {mode} in {}: {e}", repo.display()))
}

fn gitlink_violation<'a>(
    report: &'a super::ArtifactGuardReport,
    path: &str,
) -> &'a ArtifactViolation {
    report
        .violations
        .iter()
        .find(|violation| violation.path == path && violation.rule_id == GITLINK_RULE)
        .unwrap_or_else(|| {
            panic!(
                "missing {GITLINK_RULE} violation for {path:?} (mode {}); got {:#?}",
                report.mode, report.violations
            )
        })
}

fn has_gitlink_violation(report: &super::ArtifactGuardReport) -> bool {
    report
        .violations
        .iter()
        .any(|violation| violation.rule_id == GITLINK_RULE)
}

#[test]
fn run_worktree_gitlink_force_staged_in_main_checkout_blocks_pre_commit_and_pre_publish() {
    // AC4: the issue #1528 index exactly. `worktrees/` is ignored, yet a
    // forced add records the registered run worktree as a gitlink.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    force_stage_gitlink(tmp.path(), "worktrees/feat/x");
    let before = index_snapshot(tmp.path());

    for mode in [ArtifactGuardMode::PreCommit, ArtifactGuardMode::PrePublish] {
        let report = scan(tmp.path(), mode);
        let violation = gitlink_violation(&report, "worktrees/feat/x");
        assert_eq!(violation.source, ArtifactSource::Staged, "mode {mode}");
        assert_eq!(
            violation.provenance,
            ArtifactProvenance::Introduced,
            "a run-worktree gitlink is always introduced (mode {mode})"
        );
        assert!(
            report.blocks(),
            "{mode} must refuse an index holding a run-worktree gitlink; blocking: {:#?}",
            report.blocking_violations()
        );
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.path == "worktrees/feat/x" && v.rule_id == "nested-worktree"),
            "the gitlink rule takes precedence over nested-worktree (mode {mode}): {:#?}",
            report.violations
        );
    }

    assert_eq!(
        index_snapshot(tmp.path()),
        before,
        "the guard is report-only: the index must be unchanged after a scan"
    );
    assert!(
        tmp.path().join("worktrees/feat/x/README.md").exists(),
        "the guard must never touch the run worktree's files"
    );
}

#[test]
fn run_worktree_gitlink_ignores_exact_path_allowlist_entry() {
    // D9 case 1. An exact-path entry under `worktrees/` is a valid allowlist
    // entry (it exits 0 today: the gap). The gitlink rule never consults it.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    force_stage_gitlink(tmp.path(), "worktrees/feat/x");
    write_file(
        &tmp.path().join(".amplihack-artifact-allowlist"),
        "# reviewed\nworktrees/feat/x\n",
    );

    let report = scan_artifacts(
        &ArtifactGuardConfig::new(tmp.path())
            .with_mode(ArtifactGuardMode::PreCommit)
            .with_preexisting_policy(PreExistingPolicy::Report),
    )
    .expect("an exact-path allowlist entry is valid configuration, not an error");

    gitlink_violation(&report, "worktrees/feat/x");
    assert!(
        report.blocks(),
        "an allowlist entry must not authorize committing a run worktree"
    );
}

#[test]
fn run_worktree_gitlink_committed_before_baseline_still_blocks_under_report() {
    // D9 case 2. The gitlink is committed on `main` before the task branch
    // starts, so the tracked scan would call it pre-existing and `Report`
    // would downgrade it. The gitlink rule is never pre-existing.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    force_stage_gitlink(tmp.path(), "worktrees/feat/x");
    hgit(
        tmp.path(),
        &["commit", "-qm", "history: run worktree gitlink"],
    );
    hgit(tmp.path(), &["checkout", "-q", "-b", "task/unrelated"]);
    write_file(&tmp.path().join("package.json"), "{\"name\":\"x\"}\n");
    hgit(tmp.path(), &["add", "package.json"]);
    hgit(tmp.path(), &["commit", "-qm", "unrelated change"]);

    for mode in [ArtifactGuardMode::PreCommit, ArtifactGuardMode::PrePublish] {
        let report = scan_artifacts(
            &ArtifactGuardConfig::new(tmp.path())
                .with_mode(mode)
                .with_baseline("main")
                .with_preexisting_policy(PreExistingPolicy::Report),
        )
        .expect("scan artifacts");

        let violation = gitlink_violation(&report, "worktrees/feat/x");
        assert_eq!(
            violation.source,
            ArtifactSource::Tracked,
            "a committed, unstaged gitlink is reported as tracked (mode {mode})"
        );
        assert_eq!(
            violation.provenance,
            ArtifactProvenance::Introduced,
            "a run-worktree gitlink is never pre-existing (mode {mode})"
        );
        assert!(
            report.blocks(),
            "{mode} under --preexisting report must still refuse a committed gitlink"
        );
        assert!(
            !report
                .advisory_violations()
                .iter()
                .any(|v| v.path == "worktrees/feat/x"),
            "the gitlink must not also be listed as advisory (mode {mode})"
        );
    }
}

#[test]
fn registered_worktree_outside_worktrees_root_blocks_despite_covering_allowlist() {
    // D9 case 3. `wt/` is not ignored, so a plain broad add records the
    // registered worktree as a gitlink. `wt/**` is a valid allowlist entry.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "wt/review", "review");
    write_file(
        &tmp.path().join(".amplihack-artifact-allowlist"),
        "# reviewed\nwt/**\n",
    );
    hgit(tmp.path(), &["add", "-A"]);
    assert!(
        index_gitlinks(tmp.path()).contains(&"wt/review".to_string()),
        "fixture: a broad add must record the registered worktree as a gitlink"
    );

    for mode in [ArtifactGuardMode::PreCommit, ArtifactGuardMode::PrePublish] {
        let report = scan_artifacts(
            &ArtifactGuardConfig::new(tmp.path())
                .with_mode(mode)
                .with_preexisting_policy(PreExistingPolicy::Report),
        )
        .expect("a covering wt/** allowlist entry is valid configuration");
        let violation = gitlink_violation(&report, "wt/review");
        assert_eq!(violation.source, ArtifactSource::Staged);
        assert!(
            report.blocks(),
            "{mode}: a registered worktree anywhere in the repo must be refused"
        );
    }
}

#[test]
fn gitlink_for_unregistered_directory_under_worktrees_is_blocked() {
    // AC5: an aborted worktree. The directory under `worktrees/` holds a
    // partial copy and is not registered, but its gitlink is in the index.
    // Rule (a) covers `worktrees` itself and anything below it.
    for path in ["worktrees/aborted", "worktrees"] {
        let tmp = gitlink_repo();
        write_file(
            &tmp.path().join("worktrees/aborted/partial.txt"),
            "half-copied\n",
        );
        let sha = head_sha(tmp.path());
        hgit(
            tmp.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha},{path}"),
            ],
        );

        let report = scan(tmp.path(), ArtifactGuardMode::PreCommit);
        let violation = gitlink_violation(&report, path);
        assert_eq!(violation.provenance, ArtifactProvenance::Introduced);
        assert!(
            report.blocks(),
            "a gitlink at {path} must block even though nothing is registered there"
        );
    }
}

#[test]
fn mixed_case_worktrees_component_gitlink_is_blocked() {
    // SEC-8: on a case-insensitive filesystem (the macOS default)
    // `Worktrees/` is the same directory as `worktrees/`.
    for path in ["Worktrees/feat/x", "WORKTREES/feat/y", "wOrKtReEs"] {
        let tmp = gitlink_repo();
        let sha = head_sha(tmp.path());
        hgit(
            tmp.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha},{path}"),
            ],
        );

        let report = scan(tmp.path(), ArtifactGuardMode::PreCommit);
        gitlink_violation(&report, path);
        assert!(report.blocks(), "{path} must block");
    }
}

#[test]
fn only_the_first_component_selects_rule_a() {
    // Rule (a) is about the first path component. A gitlink that merely has a
    // `worktrees` component deeper down, and is not registered, is an ordinary
    // submodule-shaped entry.
    let tmp = gitlink_repo();
    let sha = head_sha(tmp.path());
    hgit(
        tmp.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{sha},vendor/worktrees/lib"),
        ],
    );

    let report = scan(tmp.path(), ArtifactGuardMode::PreCommit);
    assert!(
        !has_gitlink_violation(&report),
        "vendor/worktrees/lib is neither under the worktrees root nor registered: {:#?}",
        report.violations
    );
}

#[test]
fn real_submodule_outside_worktrees_is_not_flagged() {
    // AC6: a real submodule, listed in `.gitmodules`, outside `worktrees/` and
    // not registered as a worktree, is repository content.
    let sources = TempDir::new().expect("tempdir");
    let sub = sources.path().join("lib-src");
    fs::create_dir_all(&sub).expect("create submodule source");
    hgit(&sub, &["init", "-q"]);
    hgit(&sub, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    write_file(&sub.join("lib.rs"), "pub fn f() {}\n");
    hgit(&sub, &["add", "lib.rs"]);
    hgit(&sub, &["commit", "-qm", "lib"]);

    let tmp = gitlink_repo();
    hgit(
        tmp.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub.to_str().expect("utf-8 path"),
            "libs/sub",
        ],
    );
    assert!(index_gitlinks(tmp.path()).contains(&"libs/sub".to_string()));
    assert!(tmp.path().join(".gitmodules").exists());

    // Staged, then committed: neither form is a run worktree.
    for commit in [false, true] {
        if commit {
            hgit(tmp.path(), &["commit", "-qm", "add submodule"]);
        }
        for mode in ALL_MODES {
            let report = scan(tmp.path(), mode);
            assert!(
                !report
                    .violations
                    .iter()
                    .any(|v| v.path.starts_with("libs/sub")),
                "a real submodule must not be flagged (mode {mode}, committed {commit}): {:#?}",
                report.violations
            );
            assert!(!report.blocks(), "mode {mode}, committed {commit}");
        }
    }
}

#[test]
fn submodule_inside_linked_worktree_under_worktrees_root_is_not_flagged() {
    // K1 regression: the guard runs inside the PR's own worktree, which lives
    // at `<main>/worktrees/feat/x`. A submodule there has an absolute path under
    // the main checkout's `worktrees/`, but relative to the scanned top level it
    // is `vendor/lib`. Flagging it would refuse every submodule in every run.
    let sources = TempDir::new().expect("tempdir");
    let sub = sources.path().join("lib-src");
    fs::create_dir_all(&sub).expect("create submodule source");
    hgit(&sub, &["init", "-q"]);
    hgit(&sub, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    write_file(&sub.join("lib.rs"), "pub fn f() {}\n");
    hgit(&sub, &["add", "lib.rs"]);
    hgit(&sub, &["commit", "-qm", "lib"]);

    let main = gitlink_repo();
    register_worktree(main.path(), "worktrees/feat/x", "feat/x");
    let linked = main.path().join("worktrees/feat/x");
    hgit(
        &linked,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub.to_str().expect("utf-8 path"),
            "vendor/lib",
        ],
    );
    hgit(&linked, &["commit", "-qm", "vendor lib"]);

    for mode in ALL_MODES {
        let report = scan(&linked, mode);
        assert!(
            !has_gitlink_violation(&report),
            "a submodule inside the PR worktree is not a run worktree (mode {mode}): {:#?}",
            report.violations
        );
        assert!(!report.blocks(), "mode {mode}: {:#?}", report.violations);
    }
}

#[test]
fn clean_index_passes_pre_commit_with_registered_worktrees() {
    // AC6: live run worktrees on disk, none of them in the index.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    register_worktree(tmp.path(), "worktrees/feat/y", "feat/y");
    write_file(
        &tmp.path().join("worktrees/feat/x/target/.rustc_info.json"),
        "{}\n",
    );

    for mode in ALL_MODES {
        let report = scan(tmp.path(), mode);
        assert!(
            report.is_clean(),
            "registered, unstaged run worktrees are not violations (mode {mode}): {:#?}",
            report.violations
        );
    }
}

#[test]
fn ignored_present_inside_registered_worktree_stays_exempt_while_its_gitlink_blocks() {
    // AC6 / R6: the #857 exemption keeps hiding ignored files inside
    // registered worktrees. It never hides a gitlink.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    register_worktree(tmp.path(), "worktrees/feat/y", "feat/y");
    for wt in ["worktrees/feat/x", "worktrees/feat/y"] {
        write_file(
            &tmp.path().join(wt).join(".pytest_cache/CACHEDIR.TAG"),
            "x\n",
        );
        write_file(&tmp.path().join(wt).join("target/.rustc_info.json"), "{}\n");
    }
    force_stage_gitlink(tmp.path(), "worktrees/feat/x");

    let report = scan(tmp.path(), ArtifactGuardMode::All);

    gitlink_violation(&report, "worktrees/feat/x");
    assert!(report.blocks());
    assert!(
        !report.violations.iter().any(|v| {
            v.path.starts_with("worktrees/feat/x/") || v.path.starts_with("worktrees/feat/y")
        }),
        "ignored files inside registered worktrees stay exempt (#857): {:#?}",
        report.violations
    );
}

#[test]
fn nested_worktree_exemption_does_not_cover_untracked_scan() {
    // R6 pin: the #857 exemption is limited to the ignored-present scan. When
    // `worktrees/` is NOT ignored, a registered worktree there is an untracked
    // directory a broad add would sweep in, and it stays a violation.
    let tmp = TempDir::new().expect("tempdir");
    hgit(tmp.path(), &["init", "-q"]);
    hgit(tmp.path(), &["symbolic-ref", "HEAD", "refs/heads/main"]);
    write_file(&tmp.path().join("README.md"), "# fixture\n");
    hgit(tmp.path(), &["add", "README.md"]);
    hgit(tmp.path(), &["commit", "-qm", "initial"]);
    register_worktree(tmp.path(), "worktrees/feat/x", "feat/x");

    for mode in [ArtifactGuardMode::PreCommit, ArtifactGuardMode::All] {
        let report = scan(tmp.path(), mode);
        let violation = violation_for(
            &report.violations,
            "worktrees/feat/x",
            ArtifactSource::Untracked,
        );
        assert_eq!(violation.rule_id, "nested-worktree", "mode {mode}");
        assert!(report.blocks(), "mode {mode}");
    }
}

#[test]
fn regular_file_under_worktrees_stays_nested_worktree() {
    // Regular (non-gitlink) entries under `worktrees/` keep their existing rule.
    let tmp = gitlink_repo();
    write_file(&tmp.path().join("worktrees/notes.md"), "notes\n");
    hgit(tmp.path(), &["add", "-f", "worktrees/notes.md"]);

    let report = scan(tmp.path(), ArtifactGuardMode::PreCommit);
    let violation = violation_for(
        &report.violations,
        "worktrees/notes.md",
        ArtifactSource::Staged,
    );
    assert_eq!(violation.rule_id, "nested-worktree");
    assert!(!has_gitlink_violation(&report));
}

#[test]
fn run_worktree_gitlink_is_reported_once_and_takes_precedence_over_nested_worktree() {
    // The gitlink is both committed (tracked) and re-staged at a newer commit
    // (staged). It is one problem with one remedy: exactly one violation.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    force_stage_gitlink(tmp.path(), "worktrees/feat/x");
    hgit(tmp.path(), &["commit", "-qm", "oops: gitlink"]);
    let wt = tmp.path().join("worktrees/feat/x");
    write_file(&wt.join("change.txt"), "run work\n");
    hgit(&wt, &["add", "change.txt"]);
    hgit(&wt, &["commit", "-qm", "run work"]);
    force_stage_gitlink(tmp.path(), "worktrees/feat/x");

    for mode in ALL_MODES {
        let report = scan(tmp.path(), mode);
        let for_path: Vec<&ArtifactViolation> = report
            .violations
            .iter()
            .filter(|v| v.path == "worktrees/feat/x")
            .collect();
        assert_eq!(
            for_path.len(),
            1,
            "exactly one violation for the gitlink (mode {mode}); got {for_path:#?}"
        );
        assert_eq!(for_path[0].rule_id, GITLINK_RULE, "mode {mode}");
        assert!(report.blocks(), "mode {mode}");
    }
}

#[test]
fn conflicted_gitlink_stages_are_reported_once() {
    // SEC-10: a merge conflict gives a path stages 1-3 in the index.
    let tmp = gitlink_repo();
    let sha = head_sha(tmp.path());
    hgit_index_info(
        tmp.path(),
        &[
            ("160000", &sha, 1, "worktrees/feat/c"),
            ("160000", &sha, 2, "worktrees/feat/c"),
            ("160000", &sha, 3, "worktrees/feat/c"),
        ],
    );

    let report = scan(tmp.path(), ArtifactGuardMode::PreCommit);
    let gitlinks: Vec<&ArtifactViolation> = report
        .violations
        .iter()
        .filter(|v| v.path == "worktrees/feat/c")
        .collect();
    assert_eq!(gitlinks.len(), 1, "one violation per path: {gitlinks:#?}");
    assert_eq!(gitlinks[0].rule_id, GITLINK_RULE);
    assert!(report.blocks());
}

#[test]
fn index_records_are_split_at_the_first_tab() {
    // SEC-10: `git ls-files -s -z` prints `<mode> <sha> <stage>\t<path>`. A path
    // may itself contain a tab, so only the first tab separates the fields.
    let tmp = gitlink_repo();
    let sha = head_sha(tmp.path());
    hgit_index_info(tmp.path(), &[("160000", &sha, 0, "worktrees/a\tb")]);

    let report = scan(tmp.path(), ArtifactGuardMode::PreCommit);
    gitlink_violation(&report, "worktrees/a\tb");
    assert!(report.blocks());
}

#[test]
fn only_mode_160000_entries_are_gitlinks() {
    // SEC-10: only mode exactly 160000 is a gitlink. A symlink-mode (120000)
    // index entry sitting at a registered worktree's path is not one, even
    // though rule (b) would match its path.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "wt/review", "review");
    let blob = {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = amplihack_git::command()
            .args(["hash-object", "-w", "--stdin"])
            .current_dir(tmp.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn hash-object");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(b"target")
            .expect("write blob");
        let output = child.wait_with_output().expect("hash-object");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    hgit_index_info(tmp.path(), &[("120000", &blob, 0, "wt/review")]);

    let report = scan(tmp.path(), ArtifactGuardMode::PreCommit);
    assert!(
        !has_gitlink_violation(&report),
        "a symlink entry is not a gitlink: {:#?}",
        report.violations
    );
}

#[test]
fn prunable_registered_worktree_gitlink_is_blocked() {
    // Rule (b) includes prunable records: the worktree was registered, its
    // gitlink staged, and then its directory removed. Outside `worktrees/`,
    // only the registration identifies it.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "wt/gone", "gone");
    hgit(tmp.path(), &["add", "--", "wt/gone"]);
    assert!(index_gitlinks(tmp.path()).contains(&"wt/gone".to_string()));
    fs::remove_dir_all(tmp.path().join("wt/gone")).expect("remove worktree dir");
    assert!(
        hgit(tmp.path(), &["worktree", "list", "--porcelain"]).contains("prunable"),
        "fixture: the registration must now be prunable"
    );

    let report = scan(tmp.path(), ArtifactGuardMode::PreCommit);
    gitlink_violation(&report, "wt/gone");
    assert!(report.blocks());
}

#[test]
fn run_worktree_gitlink_blocks_in_every_mode() {
    // The rule is not gated on the staged scan: `--mode worktree` and
    // `--mode staged` report a committed gitlink too. Without the rule,
    // worktree mode would call `worktrees/feat/x` a nested-worktree (with a
    // remedy that fails on a gitlink) and would miss `wt/review` entirely.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    register_worktree(tmp.path(), "wt/review", "review");
    force_stage_gitlink(tmp.path(), "worktrees/feat/x");
    hgit(tmp.path(), &["add", "--", "wt/review"]);
    hgit(tmp.path(), &["commit", "-qm", "oops: run worktrees"]);

    for mode in ALL_MODES {
        let report = scan(tmp.path(), mode);
        for path in ["worktrees/feat/x", "wt/review"] {
            let violation = gitlink_violation(&report, path);
            assert_eq!(
                violation.source,
                ArtifactSource::Tracked,
                "{path} is committed, not staged (mode {mode})"
            );
            assert_eq!(violation.provenance, ArtifactProvenance::Introduced);
        }
        assert!(report.blocks(), "mode {mode} must block");
    }
}

#[test]
fn report_lists_registered_worktrees_inside_the_repo() {
    // The CLI uses `registered_worktrees` to never print a deleting command for
    // a root that holds a live worktree. It is filled in every mode, holds
    // repo-relative normalized paths, and excludes the scanned top level and
    // worktrees outside it.
    let tmp = gitlink_repo();
    let outside = TempDir::new().expect("tempdir");
    register_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    register_worktree(tmp.path(), "wt/review", "review");
    let outside_path = outside.path().join("elsewhere");
    hgit(
        tmp.path(),
        &[
            "worktree",
            "add",
            "-q",
            outside_path.to_str().expect("utf-8 path"),
            "-b",
            "elsewhere",
        ],
    );

    for mode in ALL_MODES {
        let report = scan(tmp.path(), mode);
        let mut listed = report.registered_worktrees.clone();
        listed.sort();
        assert_eq!(
            listed,
            vec!["worktrees/feat/x".to_string(), "wt/review".to_string()],
            "mode {mode}"
        );
    }

    // From inside a linked worktree, the main checkout is outside the scanned
    // top level and is not listed.
    let report = scan(
        &tmp.path().join("worktrees/feat/x"),
        ArtifactGuardMode::PreCommit,
    );
    assert!(
        report.registered_worktrees.is_empty(),
        "nothing is registered inside worktrees/feat/x: {:?}",
        report.registered_worktrees
    );
}

#[test]
fn gitlink_rule_is_a_named_default_rule() {
    // The CLI and the docs refer to the rule by this constant. It sits in
    // `DEFAULT_RULES` so `rule()` resolves it like every other rule, even
    // though it matches index entries rather than path patterns.
    assert_eq!(super::RUN_WORKTREE_GITLINK, GITLINK_RULE);
    assert!(
        super::DEFAULT_RULES
            .iter()
            .any(|rule| rule.id == super::RUN_WORKTREE_GITLINK),
        "DEFAULT_RULES must carry a `{GITLINK_RULE}` entry"
    );
}

#[test]
fn worktree_list_errors_display_their_fixed_payload() {
    // SEC-V6: the payload is one of three fixed strings, never Git's output,
    // so it cannot carry attacker-chosen text into the CLI's error line.
    for payload in [
        "a worktree record is not an absolute path",
        "no worktree records",
        "ambiguous worktree records",
    ] {
        let error = super::ArtifactGuardError::WorktreeList(payload.to_string());
        assert_eq!(
            error.to_string(),
            format!("artifact guard could not list worktrees: {payload}")
        );
    }
}

/// Register a worktree whose path holds a newline followed by a second
/// `worktree <abs>` line. `git worktree list --porcelain` (without `-z`) prints
/// it unquoted, so the output reads as two records, the first with no `HEAD`.
#[cfg(unix)]
fn register_forged_record_worktree(repo: &Path) {
    let top = fs::canonicalize(repo).expect("canonicalize repo");
    let forged = format!(
        "{}/x\nworktree {}/worktrees/forged",
        top.display(),
        top.display()
    );
    hgit(repo, &["worktree", "add", "-q", &forged, "-b", "forged"]);
    let listed = hgit(repo, &["worktree", "list", "--porcelain"]);
    assert!(
        listed.contains(&format!(
            "/x\nworktree {}/worktrees/forged\nHEAD ",
            top.display()
        )),
        "fixture: git must print the forged record unquoted:\n{listed}"
    );
}

#[cfg(unix)]
#[test]
fn ambiguous_worktree_records_fail_the_scan_in_every_mode() {
    // SEC-V6 / D8: a `worktree` line that arrives before the current record
    // has shown `HEAD` or `bare` is refused. A failed list is an error with no
    // partial report, even when the index also holds a gitlink.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    force_stage_gitlink(tmp.path(), "worktrees/feat/x");
    register_forged_record_worktree(tmp.path());
    let before = index_snapshot(tmp.path());

    for mode in ALL_MODES {
        match scan_artifacts(&ArtifactGuardConfig::new(tmp.path()).with_mode(mode)) {
            Err(super::ArtifactGuardError::WorktreeList(payload)) => {
                assert_eq!(payload, "ambiguous worktree records", "mode {mode}");
            }
            other => panic!("mode {mode}: expected WorktreeList(ambiguous); got {other:#?}"),
        }
    }
    assert_eq!(
        index_snapshot(tmp.path()),
        before,
        "the scan writes nothing"
    );
}

#[test]
fn a_stray_line_before_head_is_an_ambiguous_record() {
    // SEC-V6: a path `worktrees/a<LF>b` prints `worktree …/a`, then `b`, then
    // `HEAD …`. Git always follows `worktree` with `HEAD` or `bare`, so the
    // stray `b` means a path held a newline; recording `…/a` would register a
    // path that does not exist.
    let porcelain = "worktree /r\nHEAD x\nbranch refs/heads/main\n\n\
                     worktree /r/worktrees/a\nb\nHEAD y\ndetached\n\n";
    match super::gitlink::parse_worktree_porcelain(porcelain) {
        Err(super::ArtifactGuardError::WorktreeList(payload)) => {
            assert_eq!(payload, "ambiguous worktree records");
        }
        other => panic!("expected WorktreeList(ambiguous); got {other:#?}"),
    }
}

#[test]
fn optional_lines_after_head_or_bare_still_parse() {
    // The strict second-line check must not refuse the attribute lines Git
    // prints after `HEAD`/`bare`.
    let porcelain = "worktree /r\nbare\n\n\
                     worktree /r/worktrees/a\nHEAD a\nbranch refs/heads/a\nlocked\n\n\
                     worktree /r/worktrees/b\nHEAD b\ndetached\nlocked held by ci\n\n\
                     worktree /r/worktrees/c\nHEAD c\ndetached\n\
                     prunable gitdir file points to non-existent location\n\n";
    let records = super::gitlink::parse_worktree_porcelain(porcelain).expect("valid porcelain");
    let records: Vec<String> = records
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        records,
        ["/r", "/r/worktrees/a", "/r/worktrees/b", "/r/worktrees/c"]
    );
}

#[test]
fn gitlink_violations_are_listed_before_every_other_violation() {
    // D8: gitlinks sort first, so the CLI's table, its commands and its
    // `First violation detail` all lead with the one that cannot be
    // allowlisted. Every other path here sorts before `worktrees/` and `wt/`.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    register_worktree(tmp.path(), "wt/review", "review");
    force_stage_gitlink(tmp.path(), "worktrees/feat/x");
    hgit(tmp.path(), &["add", "--", "wt/review"]);
    write_file(&tmp.path().join("dist/plugin.js"), "generated\n");
    write_file(&tmp.path().join("node_modules/a/index.js"), "a\n");
    hgit(
        tmp.path(),
        &["add", "-f", "dist/plugin.js", "node_modules/a/index.js"],
    );

    for mode in [ArtifactGuardMode::PreCommit, ArtifactGuardMode::All] {
        let report = scan(tmp.path(), mode);
        let rules: Vec<&str> = report
            .violations
            .iter()
            .map(|violation| violation.rule_id.as_str())
            .collect();
        let gitlinks = rules.iter().filter(|rule| **rule == GITLINK_RULE).count();
        assert_eq!(gitlinks, 2, "mode {mode}: {rules:?}");
        assert!(
            rules[..gitlinks].iter().all(|rule| *rule == GITLINK_RULE),
            "mode {mode}: every gitlink must come before every other violation: {rules:?}"
        );
        assert!(
            rules.len() > gitlinks,
            "mode {mode}: the other violations are still reported: {rules:?}"
        );
        let first_blocking = report.blocking_violations()[0];
        assert_eq!(first_blocking.rule_id, GITLINK_RULE, "mode {mode}");
    }
}

#[test]
fn gitlink_outside_worktrees_claims_its_path_from_every_other_rule() {
    // D8: a gitlink is reported once, as `run-worktree-gitlink`, even when its
    // path also matches a path rule. `build/review` is build output by path.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "build/review", "review");
    hgit(tmp.path(), &["add", "--", "build/review"]);
    assert!(index_gitlinks(tmp.path()).contains(&"build/review".to_string()));

    for committed in [false, true] {
        if committed {
            hgit(tmp.path(), &["commit", "-qm", "oops: gitlink under build/"]);
        }
        for mode in ALL_MODES {
            let report = scan(tmp.path(), mode);
            let for_path: Vec<&ArtifactViolation> = report
                .violations
                .iter()
                .filter(|violation| violation.path == "build/review")
                .collect();
            assert_eq!(
                for_path.len(),
                1,
                "exactly one violation for build/review (mode {mode}, committed={committed}); \
                 got {for_path:#?}"
            );
            assert_eq!(for_path[0].rule_id, GITLINK_RULE, "mode {mode}");
            assert!(report.blocks(), "mode {mode}");
        }
    }
}

#[test]
fn registered_worktrees_is_sorted_deduplicated_and_includes_prunable_entries() {
    // D5: repo-relative, `/`-separated, no trailing `/`, sorted, deduplicated;
    // prunable entries included. Compared without re-sorting, so the order is
    // part of the contract.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "wt/b", "b");
    register_worktree(tmp.path(), "worktrees/feat/a", "feat/a");
    register_worktree(tmp.path(), "a/z", "z");
    fs::remove_dir_all(tmp.path().join("wt/b")).expect("remove worktree dir");
    assert!(
        hgit(tmp.path(), &["worktree", "list", "--porcelain"]).contains("prunable"),
        "fixture: wt/b must be prunable"
    );

    for mode in ALL_MODES {
        let report = scan(tmp.path(), mode);
        assert_eq!(
            report.registered_worktrees,
            vec![
                "a/z".to_string(),
                "worktrees/feat/a".to_string(),
                "wt/b".to_string()
            ],
            "mode {mode}"
        );
    }
}

#[test]
fn prunable_worktree_under_worktrees_keeps_its_ignored_files_exempt() {
    // D5: the #857 nested-worktree exemption now comes from
    // `registered_worktrees`, which includes prunable entries. A registered
    // worktree whose `.git` file went missing (prunable) still holds a run's
    // files; they stay exempt in the ignored-present scan.
    let tmp = gitlink_repo();
    register_worktree(tmp.path(), "worktrees/feat/p", "feat/p");
    write_file(
        &tmp.path().join("worktrees/feat/p/node_modules/a/index.js"),
        "a\n",
    );
    fs::remove_file(tmp.path().join("worktrees/feat/p/.git")).expect("remove gitfile");
    assert!(
        hgit(tmp.path(), &["worktree", "list", "--porcelain"]).contains("prunable"),
        "fixture: worktrees/feat/p must be prunable"
    );

    let report = scan(tmp.path(), ArtifactGuardMode::All);
    assert_eq!(
        report.registered_worktrees,
        vec!["worktrees/feat/p".to_string()]
    );
    let inside: Vec<&ArtifactViolation> = report
        .violations
        .iter()
        .filter(|violation| violation.path.starts_with("worktrees/feat/p"))
        .collect();
    assert!(
        inside.is_empty(),
        "files inside a registered (prunable) worktree are exempt: {inside:#?}"
    );
}
