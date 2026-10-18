//! tests/integration/artifact_guard_cli_tests.rs
//!
//! Contracts for the `amplihack hygiene artifact-guard` CLI.
//!
//! The command must scan the whole repository state, return exit code 1 for
//! artifact violations, return exit code 2 for configuration/Git failures, and
//! print actionable remediation without deleting artifacts.

use amplihack_cli::Cli;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_amplihack")
}

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

#[test]
fn hygiene_artifact_guard_cli_surface_parses_repo_mode_and_allowlist() {
    let parsed = Cli::try_parse_from([
        "amplihack",
        "hygiene",
        "artifact-guard",
        "--repo",
        "/tmp/example-repo",
        "--mode",
        "pre-commit",
        "--allowlist",
        "/tmp/example-repo/.amplihack-artifact-allowlist",
    ]);

    assert!(
        parsed.is_ok(),
        "hygiene artifact-guard must parse --repo, --mode, and --allowlist: {parsed:?}"
    );
}

#[test]
fn artifact_guard_help_documents_exit_codes_and_remediation_behavior() {
    let error = Cli::try_parse_from(["amplihack", "hygiene", "artifact-guard", "--help"])
        .expect_err("clap help exits through an error-like display result");
    let help = error.to_string();

    for required in [
        "pre-commit",
        "pre-publish",
        "exit code 0",
        "exit code 1",
        "exit code 2",
        "does not delete",
        "allowlist",
    ] {
        assert!(
            help.contains(required),
            "artifact-guard help must document `{required}`; got:\n{help}"
        );
    }
}

#[test]
fn cli_returns_exit_1_with_paths_and_remediation_for_artifact_violations() {
    let tmp = repo();
    write_file(
        &tmp.path().join("dist/plugin.js"),
        "generated plugin bundle\n",
    );

    let output = Command::new(bin())
        .args([
            "hygiene",
            "artifact-guard",
            "--mode",
            "pre-commit",
            "--repo",
        ])
        .arg(tmp.path())
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("run artifact guard");

    assert_eq!(
        output.status.code(),
        Some(1),
        "artifact violations must exit 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("dist/plugin.js"),
        "must print repo-relative path"
    );
    assert!(
        combined.contains("remove") && combined.contains("outside the parent worktree"),
        "must print clear remediation; got:\n{combined}"
    );
    assert!(
        tmp.path().join("dist/plugin.js").exists(),
        "CLI guard must not silently delete artifacts"
    );
}

#[test]
fn cli_returns_exit_0_for_clean_repository() {
    let tmp = repo();

    let output = Command::new(bin())
        .args([
            "hygiene",
            "artifact-guard",
            "--mode",
            "pre-commit",
            "--repo",
        ])
        .arg(tmp.path())
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("run artifact guard");

    assert!(
        output.status.success(),
        "clean repos must exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_exits_0_when_only_launcher_owned_runtime_files_are_present() {
    // Regression for issue #807: the launcher's own `.claude/runtime/` files
    // must not turn the end-of-run guard into a non-zero exit (which hung the
    // runner). The pre-publish guard over a worktree that only contains launcher
    // bookkeeping must return a clean exit 0.
    let tmp = repo();
    write_file(&tmp.path().join(".gitignore"), ".claude/runtime/\n");
    run_git(tmp.path(), &["add", ".gitignore"]);
    run_git(tmp.path(), &["commit", "-qm", "ignore claude runtime"]);
    write_file(
        &tmp.path().join(".claude/runtime/launcher_context.json"),
        "{}\n",
    );
    write_file(&tmp.path().join(".claude/runtime/sessions.jsonl"), "{}\n");

    let output = Command::new(bin())
        .args([
            "hygiene",
            "artifact-guard",
            "--mode",
            "pre-publish",
            "--repo",
        ])
        .arg(tmp.path())
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("run artifact guard");

    assert!(
        output.status.success(),
        "launcher-owned runtime files must not block the guard\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_exempts_ignored_present_claude_runtime_but_blocks_when_staged() {
    // Source-conditional contract for the whole `.claude/runtime/` subtree:
    //
    //   * Ignored-present (or untracked) runtime output can NEVER enter the
    //     deliverable, so it must not block a publish — this is the exact
    //     false-positive that previously hung recipe-runner on gitignored
    //     runtime metrics/session files.
    //   * A *staged* `.claude/runtime/` file could actually be committed, so it
    //     must still fail closed as genuine pollution entering the deliverable.
    //
    // Uses `--mode worktree` for the ignored-present half because issue #928
    // narrowed `pre-publish` so it no longer scans ignored-present paths at all;
    // worktree is the mode that still audits ignored-present state, so it is the
    // strongest place to prove the exemption.
    let tmp = repo();
    write_file(&tmp.path().join(".gitignore"), ".claude/runtime/\n");
    run_git(tmp.path(), &["add", ".gitignore"]);
    run_git(tmp.path(), &["commit", "-qm", "ignore claude runtime"]);
    write_file(
        &tmp.path().join(".claude/runtime/launcher_context.json"),
        "{}\n",
    );
    write_file(&tmp.path().join(".claude/runtime/session.json"), "{}\n");

    let output = run_guard(tmp.path(), "worktree");

    assert_eq!(
        output.status.code(),
        Some(0),
        "ignored-present .claude/runtime/ output must not block (it can never \
         enter the deliverable)\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Second half: force-stage the same runtime file so it becomes committable.
    // A staged `.claude/runtime/` path is deliberate pollution entering the
    // deliverable and must still fail closed under pre-commit.
    run_git(tmp.path(), &["add", "-f", ".claude/runtime/session.json"]);

    let staged = run_guard(tmp.path(), "pre-commit");

    assert_eq!(
        staged.status.code(),
        Some(1),
        "staged .claude/runtime/ file must still block pre-commit\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&staged.stdout),
        String::from_utf8_lossy(&staged.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&staged.stdout),
        String::from_utf8_lossy(&staged.stderr)
    );
    assert!(
        combined.contains(".claude/runtime/session.json"),
        "must report the staged runtime pollution; got:\n{combined}"
    );
    assert!(
        !combined.contains("launcher_context.json"),
        "must not report the exempt launcher file; got:\n{combined}"
    );
}

#[test]
fn cli_returns_exit_2_for_invalid_allowlist() {
    let tmp = repo();
    let allowlist = tmp.path().join(".amplihack-artifact-allowlist");
    write_file(&allowlist, "node_modules/**\n");

    let output = Command::new(bin())
        .args([
            "hygiene",
            "artifact-guard",
            "--mode",
            "pre-commit",
            "--repo",
        ])
        .arg(tmp.path())
        .arg("--allowlist")
        .arg(&allowlist)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("run artifact guard");

    assert_eq!(
        output.status.code(),
        Some(2),
        "invalid allowlist must exit 2\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("allowlist") && stderr.contains("unsafe"),
        "invalid allowlist error must be clear; got:\n{stderr}"
    );
}

/// Seed a repo whose gitignored `.pytest_cache/` and `node_modules/` cache
/// directories are present + untracked. These can never be committed or
/// published, so pre-commit/pre-publish must not fail-closed on them (#928).
fn repo_with_ignored_present_cache() -> TempDir {
    let tmp = repo();
    write_file(
        &tmp.path().join(".gitignore"),
        ".pytest_cache/\nnode_modules/\n",
    );
    run_git(tmp.path(), &["add", ".gitignore"]);
    run_git(tmp.path(), &["commit", "-qm", "ignore cache artifacts"]);
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

fn run_guard(repo: &Path, mode: &str) -> std::process::Output {
    Command::new(bin())
        .args(["hygiene", "artifact-guard", "--mode", mode, "--repo"])
        .arg(repo)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("run artifact guard")
}

#[test]
fn cli_pre_commit_exits_0_for_ignored_present_cache_artifacts() {
    // Issue #928: gitignored+present cache dirs (.pytest_cache/, node_modules/)
    // can never be committed, so the pre-commit guard must exit 0.
    let tmp = repo_with_ignored_present_cache();

    let output = run_guard(tmp.path(), "pre-commit");

    assert_eq!(
        output.status.code(),
        Some(0),
        "ignored-present cache must not block pre-commit (#928)\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        tmp.path().join("node_modules/leftpad/index.js").exists(),
        "guard must not delete ignored cache artifacts"
    );
}

#[test]
fn cli_pre_publish_exits_0_for_ignored_present_cache_artifacts() {
    // Issue #928: the pre-publish guard is fail-closed for anything that could be
    // published; gitignored+present cache can never be, so it must exit 0.
    let tmp = repo_with_ignored_present_cache();

    let output = run_guard(tmp.path(), "pre-publish");

    assert_eq!(
        output.status.code(),
        Some(0),
        "ignored-present cache must not block pre-publish (#928)\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_worktree_still_exits_1_for_ignored_present_cache_artifacts() {
    // Regression guard: the #928 narrowing is scoped to pre-commit/pre-publish.
    // The full worktree audit must still surface ignored-present cache leaks.
    let tmp = repo_with_ignored_present_cache();

    let output = run_guard(tmp.path(), "worktree");

    assert_eq!(
        output.status.code(),
        Some(1),
        "worktree mode must still flag ignored-present cache\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_pre_commit_still_exits_1_for_staged_committable_artifact() {
    // Regression guard: a *staged* prohibited artifact could actually be committed
    // and must still fail closed under the narrowed pre-commit mode.
    let tmp = repo();
    write_file(
        &tmp.path().join("node_modules/leak/index.js"),
        "module.exports = 1;\n",
    );
    run_git(tmp.path(), &["add", "-f", "node_modules/leak/index.js"]);

    let output = run_guard(tmp.path(), "pre-commit");

    assert_eq!(
        output.status.code(),
        Some(1),
        "staged committable artifact must still block pre-commit\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("node_modules/leak/index.js"),
        "must report the staged committable artifact; got:\n{combined}"
    );
}

// ---------------------------------------------------------------------------
// Issue #1422: the CLI must answer a pre-existing repository condition with a
// full report and exit 0, and answer artifacts the change itself introduced
// with a refusal that names the paths and the exact commands.
// ---------------------------------------------------------------------------

/// A repo shaped like the one in issue #1422: `node_modules` committed on the
/// base branch long ago, and a task branch carrying an unrelated change.
fn repo_with_preexisting_tracked_node_modules() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    run_git(tmp.path(), &["init", "-q"]);
    run_git(tmp.path(), &["symbolic-ref", "HEAD", "refs/heads/main"]);
    run_git(
        tmp.path(),
        &["config", "user.email", "artifact-guard@example.invalid"],
    );
    run_git(tmp.path(), &["config", "user.name", "Artifact Guard Test"]);
    write_file(&tmp.path().join("README.md"), "# fixture\n");
    run_git(tmp.path(), &["add", "README.md"]);
    run_git(tmp.path(), &["commit", "-qm", "initial"]);
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

fn combined(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn cli_pre_publish_exits_0_and_reports_a_preexisting_condition_with_a_way_forward() {
    let tmp = repo_with_preexisting_tracked_node_modules();

    let output = run_guard(tmp.path(), "pre-publish");
    let combined = combined(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "a pre-existing repository condition this change did not create must not \
         discard the change's verified work (#1422)\n{combined}"
    );
    for required in [
        "pre-existing",
        "NOT blocking",
        "node_modules/leftpad/index.js",
        "git rm -r --cached -- 'node_modules'",
        "--mode all",
        "--preexisting block",
    ] {
        assert!(
            combined.contains(required),
            "the pre-existing report must name the paths and the way forward, missing \
             `{required}`; got:\n{combined}"
        );
    }
    assert!(
        tmp.path().join("node_modules/leftpad/index.js").exists(),
        "the guard must never delete anything"
    );
}

#[test]
fn cli_pre_publish_exits_1_and_names_paths_for_artifacts_this_change_introduced() {
    let tmp = repo_with_preexisting_tracked_node_modules();
    // The change under review commits its own artifact tree on the task branch.
    write_file(&tmp.path().join("dist/plugin.js"), "generated bundle\n");
    run_git(tmp.path(), &["add", "-f", "dist/plugin.js"]);
    run_git(tmp.path(), &["commit", "-qm", "oops: commit built bundle"]);

    let output = run_guard(tmp.path(), "pre-publish");
    let combined = combined(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "artifacts this change introduced must still fail closed\n{combined}"
    );
    for required in [
        "dist/plugin.js",
        "introduced",
        "git rm -r --cached -- 'dist'",
        "re-run to confirm",
    ] {
        assert!(
            combined.contains(required),
            "the refusal must name the offending path and the exact remedy, missing \
             `{required}`; got:\n{combined}"
        );
    }
    assert!(
        !combined.contains("Artifact Guard blocked 3 ")
            && !combined.contains("Artifact Guard blocked 2 "),
        "only the introduced path may be counted as blocking, not the pre-existing \
         node_modules tree; got:\n{combined}"
    );
}

#[test]
fn cli_preexisting_block_policy_restores_the_old_fail_closed_gate() {
    let tmp = repo_with_preexisting_tracked_node_modules();

    let flag = Command::new(bin())
        .args([
            "hygiene",
            "artifact-guard",
            "--mode",
            "pre-publish",
            "--preexisting",
            "block",
            "--repo",
        ])
        .arg(tmp.path())
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("run artifact guard");
    assert_eq!(
        flag.status.code(),
        Some(1),
        "--preexisting block must fail closed\n{}",
        combined(&flag)
    );

    let env = Command::new(bin())
        .args([
            "hygiene",
            "artifact-guard",
            "--mode",
            "pre-publish",
            "--repo",
        ])
        .arg(tmp.path())
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_ARTIFACT_GUARD_PREEXISTING", "block")
        .output()
        .expect("run artifact guard");
    assert_eq!(
        env.status.code(),
        Some(1),
        "AMPLIHACK_ARTIFACT_GUARD_PREEXISTING=block must fail closed\n{}",
        combined(&env)
    );
}

#[test]
fn cli_mode_all_still_fails_closed_on_a_preexisting_condition() {
    // The condition never stops being reported: `--mode all` is the audit that
    // lists it in full and exits 1, which is the documented way to see it.
    let tmp = repo_with_preexisting_tracked_node_modules();

    let output = run_guard(tmp.path(), "all");
    let combined = combined(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "the audit mode must keep failing closed on the whole repository condition\n{combined}"
    );
    assert!(
        combined.contains("node_modules/leftpad/index.js"),
        "the audit must list every offending path; got:\n{combined}"
    );
}

#[test]
fn cli_rejects_an_unresolvable_explicit_baseline_with_exit_2() {
    let tmp = repo_with_preexisting_tracked_node_modules();

    let output = Command::new(bin())
        .args([
            "hygiene",
            "artifact-guard",
            "--mode",
            "pre-publish",
            "--baseline",
            "refs/heads/does-not-exist",
            "--repo",
        ])
        .arg(tmp.path())
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("run artifact guard");

    assert_eq!(
        output.status.code(),
        Some(2),
        "an explicitly requested baseline that cannot resolve is a misconfiguration, \
         not a silent fallback\n{}",
        combined(&output)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("baseline `refs/heads/does-not-exist`"),
        "the error must name the baseline that failed to resolve; got:\n{stderr}"
    );
}

#[test]
fn cli_never_prints_a_remedy_that_would_untrack_the_preexisting_tree() {
    // The change adds one package inside a vendored tree of many. Collapsing the
    // remedy to `git rm -r --cached node_modules` would untrack all of them --
    // exactly the unrelated, out-of-scope change issue #1422 says the run must
    // not be pushed into making.
    let tmp = repo_with_preexisting_tracked_node_modules();
    write_file(
        &tmp.path().join("node_modules/newdep/index.js"),
        "module.exports = 1;\n",
    );
    run_git(tmp.path(), &["add", "-f", "node_modules/newdep/index.js"]);
    run_git(tmp.path(), &["commit", "-qm", "add one new package"]);

    let output = run_guard(tmp.path(), "pre-publish");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    let (_, refusal) = combined
        .split_once("Exact commands for the paths above:")
        .unwrap_or_else(|| panic!("refusal must print exact commands; got:\n{combined}"));
    assert!(
        refusal.contains("git rm --cached -- 'node_modules/newdep/index.js'"),
        "the remedy must name the introduced path exactly; got:\n{refusal}"
    );
    assert!(
        !refusal.contains("git rm -r --cached -- 'node_modules'"),
        "the refusal's remedy must never sweep the pre-existing tree along with the \
         one path this change added; got:\n{refusal}"
    );
}

// ---------------------------------------------------------------------------
// Issue #1536: a tracked `apps/coverage/` application was flagged as a build
// artifact and the report told the operator to untrack it -- deleting source
// from the index. The CLI must pass evidently-source directories, never print
// a removal command for a path that may be source (it points at the allowlist
// instead), and not charge a base-branch merge to the PR.
// ---------------------------------------------------------------------------

/// A repository on `main` with a task branch checked out.
fn repo_on_task_branch() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    run_git(tmp.path(), &["init", "-q"]);
    run_git(tmp.path(), &["symbolic-ref", "HEAD", "refs/heads/main"]);
    run_git(
        tmp.path(),
        &["config", "user.email", "artifact-guard@example.invalid"],
    );
    run_git(tmp.path(), &["config", "user.name", "Artifact Guard Test"]);
    write_file(&tmp.path().join("README.md"), "# fixture\n");
    run_git(tmp.path(), &["add", "README.md"]);
    run_git(tmp.path(), &["commit", "-qm", "initial"]);
    run_git(tmp.path(), &["checkout", "-q", "-b", "task/change"]);
    tmp
}

fn commit_paths(repo: &Path, paths: &[(&str, &str)], message: &str) {
    for (path, content) in paths {
        write_file(&repo.join(path), content);
        run_git(repo, &["add", "-f", "--", path]);
    }
    run_git(repo, &["commit", "-qm", message]);
}

/// No command in the output removes anything from the index or the disk.
fn assert_no_removal_commands(combined: &str) {
    for forbidden in ["rm -r --cached", "rm --cached", "rm -rf"] {
        assert!(
            !combined.contains(forbidden),
            "a path that may be source must never get `{forbidden}`; got:\n{combined}"
        );
    }
}

#[test]
fn cli_passes_a_tracked_app_named_coverage() {
    let tmp = repo_on_task_branch();
    commit_paths(
        tmp.path(),
        &[
            ("apps/coverage/package.json", "{\"name\":\"coverage\"}\n"),
            ("apps/coverage/tsconfig.json", "{}\n"),
            ("apps/coverage/src/index.ts", "export const x = 1;\n"),
        ],
        "add the coverage app",
    );

    for mode in ["pre-commit", "pre-publish"] {
        let output = run_guard(tmp.path(), mode);
        let combined = combined(&output);
        assert_eq!(
            output.status.code(),
            Some(0),
            "tracked apps/coverage source must pass {mode}\n{combined}"
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("apps/coverage"),
            "{mode} must not report apps/coverage at all; got:\n{combined}"
        );
    }
}

#[test]
fn cli_still_blocks_a_staged_coverage_report_with_an_exact_remedy() {
    let tmp = repo_on_task_branch();
    write_file(&tmp.path().join("coverage/lcov.info"), "TN:\n");
    run_git(tmp.path(), &["add", "-f", "coverage/lcov.info"]);

    let output = run_guard(tmp.path(), "pre-commit");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    for required in [
        "coverage/lcov.info",
        "build-artifact",
        "git rm -r --cached -- 'coverage/lcov.info'",
    ] {
        assert!(
            combined.contains(required),
            "missing `{required}`; got:\n{combined}"
        );
    }
    assert!(
        !combined.contains("-- 'coverage'"),
        "the remedy must name the generated file, not its directory; got:\n{combined}"
    );
}

#[test]
fn cli_still_blocks_a_tracked_dist_bundle_next_to_a_root_manifest() {
    let tmp = repo_on_task_branch();
    commit_paths(
        tmp.path(),
        &[
            ("package.json", "{\"name\":\"root\"}\n"),
            ("dist/app.js", "bundle\n"),
        ],
        "oops: commit the bundle",
    );

    let output = run_guard(tmp.path(), "pre-publish");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    assert!(
        combined.contains("dist/app.js") && combined.contains("build-artifact"),
        "a root package.json must not exempt dist/; got:\n{combined}"
    );
}

#[test]
fn cli_points_a_tracked_ambiguous_path_at_the_allowlist_instead_of_untracking_it() {
    let tmp = repo_on_task_branch();
    commit_paths(tmp.path(), &[("build/notes.md", "# notes\n")], "notes");

    let output = run_guard(tmp.path(), "pre-publish");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    let (_, remedy) = combined
        .split_once("Exact commands for the paths above:")
        .unwrap_or_else(|| panic!("refusal must print its remedy section; got:\n{combined}"));
    assert!(
        remedy.contains(".amplihack-artifact-allowlist") && remedy.contains("build/notes.md"),
        "the remedy must suggest an allowlist entry for the exact path; got:\n{remedy}"
    );
    assert_no_removal_commands(&combined);
    let (_, detail) = combined
        .split_once("First violation detail:")
        .unwrap_or_else(|| {
            panic!("refusal must print its first-violation detail; got:\n{combined}")
        });
    assert!(
        detail.contains("build/notes.md") && detail.contains(".amplihack-artifact-allowlist"),
        "the first-violation detail must point the possibly-source path at review; got:\n{detail}"
    );
    assert!(
        !combined.contains("remove the artifact"),
        "no line may tell the reader to remove a path the guard calls possibly source; got:\n{combined}"
    );

    // The suggested exact-path entry is one the allowlist parser accepts.
    write_file(
        &tmp.path().join(".amplihack-artifact-allowlist"),
        "build/notes.md\n",
    );
    let rerun = run_guard(tmp.path(), "pre-publish");
    assert_eq!(
        rerun.status.code(),
        Some(0),
        "the suggested allowlist entry must clear the path\n{}{}",
        String::from_utf8_lossy(&rerun.stdout),
        String::from_utf8_lossy(&rerun.stderr)
    );
}

#[test]
fn cli_advisory_report_never_suggests_untracking_a_preexisting_ambiguous_path() {
    let tmp = TempDir::new().expect("tempdir");
    run_git(tmp.path(), &["init", "-q"]);
    run_git(tmp.path(), &["symbolic-ref", "HEAD", "refs/heads/main"]);
    run_git(
        tmp.path(),
        &["config", "user.email", "artifact-guard@example.invalid"],
    );
    run_git(tmp.path(), &["config", "user.name", "Artifact Guard Test"]);
    commit_paths(tmp.path(), &[("build/notes.md", "# notes\n")], "notes");
    run_git(tmp.path(), &["checkout", "-q", "-b", "task/change"]);
    commit_paths(tmp.path(), &[("src/lib.rs", "pub fn f() {}\n")], "work");

    let output = run_guard(tmp.path(), "pre-publish");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(0), "{combined}");
    assert!(
        combined.contains("build/notes.md") && combined.contains(".amplihack-artifact-allowlist"),
        "the advisory report must name the path and the allowlist; got:\n{combined}"
    );
    assert_no_removal_commands(&combined);
    assert!(
        !combined.contains(">> .gitignore"),
        "no untrack-and-ignore chain for a path that may be source; got:\n{combined}"
    );
}

#[test]
fn cli_names_the_nested_node_modules_root_never_the_app_directory() {
    let tmp = repo_on_task_branch();
    commit_paths(
        tmp.path(),
        &[("apps/coverage/package.json", "{}\n")],
        "coverage app",
    );
    write_file(
        &tmp.path().join("apps/coverage/node_modules/a/index.js"),
        "module.exports = 1;\n",
    );

    let output = run_guard(tmp.path(), "pre-commit");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    assert!(
        combined.contains("-- 'apps/coverage/node_modules'"),
        "the remedy must target the node_modules tree; got:\n{combined}"
    );
    assert!(
        !combined.contains("-- 'apps/coverage'"),
        "the remedy must never target the application directory; got:\n{combined}"
    );
}

#[test]
fn cli_groups_output_inside_an_exempt_app_under_the_output_directory() {
    // Found in outside-in testing: the review section headed this group
    // `apps/coverage/`, naming the exempt application as the artifact.
    let tmp = repo_on_task_branch();
    commit_paths(
        tmp.path(),
        &[
            ("apps/coverage/package.json", "{}\n"),
            ("apps/coverage/dist/main.js", "x\n"),
        ],
        "coverage app with a committed bundle",
    );

    let output = run_guard(tmp.path(), "pre-publish");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    assert!(
        combined.contains("  apps/coverage/dist/  — 1 path(s)"),
        "the group must be headed by the dist directory; got:\n{combined}"
    );
    assert!(
        !combined.contains("  apps/coverage/  —"),
        "the group must never be headed by the application directory; got:\n{combined}"
    );
    assert_no_removal_commands(&combined);
}

#[test]
fn cli_shared_root_containment_respects_the_segment_boundary() {
    // `buildtools/out/a.txt` is a tracked, possibly-source path; it is not
    // inside `build`, so the untracked `build/` leftover keeps its whole-root
    // remedy and nothing under `buildtools` is ever removed.
    let tmp = repo_on_task_branch();
    commit_paths(tmp.path(), &[("buildtools/out/a.txt", "a\n")], "tools");
    write_file(&tmp.path().join("build/x.o"), "obj\n");

    let output = run_guard(tmp.path(), "pre-publish");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    assert!(
        combined.contains("rm -rf -- 'build'"),
        "the untracked build/ leftover keeps its root remedy; got:\n{combined}"
    );
    assert!(
        !combined.contains("rm -r --cached -- 'build'"),
        "nothing under build/ is indexed, so git rm --cached would fail; got:\n{combined}"
    );
    for line in combined.lines().filter(|line| line.contains("rm ")) {
        assert!(
            !line.contains("buildtools"),
            "no removal command may touch buildtools: {line}"
        );
    }
}

#[test]
fn cli_never_prints_a_removal_command_for_a_root_holding_a_quote() {
    let tmp = repo_on_task_branch();
    write_file(&tmp.path().join("it's/build/x.o"), "obj\n");

    let output = run_guard(tmp.path(), "pre-commit");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    assert!(
        combined.contains("x.o"),
        "the path must still be reported; got:\n{combined}"
    );
    assert_no_removal_commands(&combined);
}

#[test]
fn cli_never_prints_a_per_path_command_for_a_path_holding_a_quote() {
    // The root `node_modules` is shell-safe and shared with pre-existing
    // entries, so the guard lists per path; the path itself must still not be
    // interpolated into a single-quoted shell word.
    let tmp = repo_with_preexisting_tracked_node_modules();
    let name = "node_modules/x'; touch PWNED; echo '/index.js";
    write_file(&tmp.path().join(name), "y\n");
    run_git(tmp.path(), &["add", "-f", "node_modules"]);
    run_git(tmp.path(), &["commit", "-qm", "quote-shaped package"]);

    let output = run_guard(tmp.path(), "pre-publish");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    assert!(
        !combined.contains("touch PWNED; echo '/index.js'"),
        "a quote in the path must not reach a shell command; got:\n{combined}"
    );
    assert!(
        combined.contains("unsafe in a shell command"),
        "the path must be routed to manual handling; got:\n{combined}"
    );
}

#[test]
fn cli_gives_a_glob_shaped_tracked_path_no_allowlist_line() {
    let tmp = repo_on_task_branch();
    let name = "build/a*b.txt";
    if fs::write(tmp.path().join("probe*name"), "").is_err() {
        // The filesystem rejects `*` in names; nothing to test here.
        return;
    }
    fs::remove_file(tmp.path().join("probe*name")).expect("remove probe");
    commit_paths(tmp.path(), &[(name, "a\n")], "glob-shaped name");

    let output = run_guard(tmp.path(), "pre-publish");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    assert!(
        combined.contains("a*b.txt") && combined.contains("manual review"),
        "a path with no literal allowlist form needs manual review; got:\n{combined}"
    );
    assert_no_removal_commands(&combined);
}

#[test]
fn cli_does_not_attribute_a_base_branch_merge_to_the_pr() {
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
    commit_paths(&work, &[("README.md", "# fixture\n")], "initial");
    run_git(&work, &["push", "-q", "-u", "origin", "main"]);

    run_git(&work, &["checkout", "-q", "-b", "feature"]);
    commit_paths(&work, &[("src/lib.rs", "pub fn f() {}\n")], "feature work");
    run_git(&work, &["push", "-q", "-u", "origin", "feature"]);

    run_git(&work, &["checkout", "-q", "main"]);
    commit_paths(&work, &[("build/out.log", "log\n")], "main: build log");
    run_git(&work, &["push", "-q", "origin", "main"]);

    run_git(&work, &["checkout", "-q", "feature"]);
    run_git(&work, &["fetch", "-q", "origin"]);
    run_git(
        &work,
        &["merge", "-q", "--no-ff", "-m", "merge main", "origin/main"],
    );
    run_git(&work, &["remote", "set-head", "origin", "main"]);

    let output = run_guard(&work, "pre-publish");
    let combined = combined(&output);

    assert_eq!(
        output.status.code(),
        Some(0),
        "a path the base branch merged in is not the PR's change\n{combined}"
    );
    assert!(
        combined.contains("origin/HEAD"),
        "the report must name the base-branch baseline; got:\n{combined}"
    );
}

#[test]
fn cli_help_lists_the_base_branch_before_upstream_in_the_baseline_order() {
    let error = Cli::try_parse_from(["amplihack", "hygiene", "artifact-guard", "--help"])
        .expect_err("clap help exits through an error-like display result");
    let help = error.to_string();

    let base = help
        .find("origin/HEAD")
        .unwrap_or_else(|| panic!("help must name origin/HEAD; got:\n{help}"));
    let upstream = help
        .find("@{upstream}")
        .unwrap_or_else(|| panic!("help must name @{{upstream}}; got:\n{help}"));
    assert!(
        base < upstream,
        "origin/HEAD must come before @{{upstream}} in the baseline order; got:\n{help}"
    );
}

#[test]
fn cli_never_suggests_an_allowlist_entry_the_parser_rejects() {
    let tmp = repo_on_task_branch();
    write_file(&tmp.path().join("build"), "#!/bin/sh\necho build\n");
    run_git(tmp.path(), &["add", "-f", "build"]);

    let output = run_guard(tmp.path(), "pre-commit");
    let report = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{report}");

    let suggested: Vec<&str> = report
        .split_once("add a reviewed entry to .amplihack-artifact-allowlist:")
        .map(|(_, rest)| {
            rest.lines()
                .skip(1)
                .take_while(|line| line.starts_with("      "))
                .map(str::trim)
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !suggested.contains(&"build"),
        "`build` is refused by the allowlist parser and must not be suggested; got:\n{report}"
    );
    let refusal = "\"build\" cannot be allowlisted";
    assert_eq!(
        report.matches(refusal).count(),
        1,
        "the refused path must be named exactly once; got:\n{report}"
    );
    let (_, remedy) = report
        .split_once("Exact commands for the paths above:")
        .unwrap_or_else(|| panic!("refusal must print its remedy section; got:\n{report}"));
    let listed = remedy
        .split_once("First violation detail:")
        .map_or(remedy, |(head, _)| head)
        .lines()
        .filter(|line| line.trim() == "build")
        .count();
    assert!(
        listed <= 1,
        "a path reported as both staged and tracked must be listed once; got:\n{report}"
    );

    // Following every suggestion must never turn the violation into a config error.
    if !suggested.is_empty() {
        write_file(
            &tmp.path().join(".amplihack-artifact-allowlist"),
            &format!("{}\n", suggested.join("\n")),
        );
        let rerun = run_guard(tmp.path(), "pre-commit");
        let rerun_combined = combined(&rerun);
        assert_ne!(rerun.status.code(), Some(2), "{rerun_combined}");
        assert!(
            !rerun_combined.contains("unsafe allowlist entry"),
            "{rerun_combined}"
        );
    }
}

#[test]
fn cli_never_suggests_removing_a_directory_that_holds_allowlisted_source() {
    // Review repro on PR #1587: the allowlisted file is never a violation, so
    // only the index can tell the guard that `docs/build` holds source.
    let tmp = repo_on_task_branch();
    commit_paths(
        tmp.path(),
        &[
            ("docs/build/guide.md", "# guide\n"),
            (".amplihack-artifact-allowlist", "docs/build/guide.md\n"),
        ],
        "docs",
    );
    write_file(&tmp.path().join("docs/build/scratch.log"), "x\n");

    let output = run_guard(tmp.path(), "pre-commit");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    assert!(
        combined.contains("rm -rf -- 'docs/build/scratch.log'"),
        "the remedy must name the leftover exactly; got:\n{combined}"
    );
    for line in combined.lines().filter(|line| line.contains("rm ")) {
        assert!(
            !line.contains("'docs/build'"),
            "no removal command may sweep the allowlisted source: {line}"
        );
        assert!(
            !line.contains("--cached"),
            "an untracked leftover must not get a git rm that fails: {line}"
        );
    }
}

/// Assert the report prints the literal-pathspec `git rm -r --cached` for
/// `root`, run that exact command with `--dry-run`, and return the paths git
/// says it would untrack.
fn dry_run_printed_untrack(repo: &Path, combined: &str, root: &str) -> Vec<String> {
    let printed = format!("git --literal-pathspecs rm -r --cached -- '{root}'");
    assert!(
        combined.contains(&printed),
        "the remedy must untrack {root:?} as a literal path; got:\n{combined}"
    );
    let output = amplihack_git::command()
        .args([
            "--literal-pathspecs",
            "rm",
            "-r",
            "--cached",
            "--dry-run",
            "--",
            root,
        ])
        .current_dir(repo)
        .output()
        .expect("run git rm --dry-run");
    assert!(
        output.status.success(),
        "the printed command must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn cli_untracks_a_pathspec_magic_root_as_one_literal_path() {
    // Without `--literal-pathspecs`, `git rm -r --cached -- ':!src/node_modules'`
    // would untrack every file in the repository: git reads `:!` as
    // "everything except", even after `--`.
    let tmp = repo_on_task_branch();
    write_file(&tmp.path().join(":!src/node_modules/x/index.js"), "y\n");
    run_git(tmp.path(), &["add", "-f", "--", ":(literal):!src"]);

    let output = run_guard(tmp.path(), "pre-commit");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    let removed = dry_run_printed_untrack(tmp.path(), &combined, ":!src/node_modules");
    assert_eq!(
        removed,
        vec!["rm ':!src/node_modules/x/index.js'".to_string()],
        "the printed command must untrack only the leaked tree; got:\n{combined}"
    );
}

#[test]
fn cli_untracks_a_glob_named_root_as_one_literal_path() {
    // `pkg[1]` read as a glob also matches `pkg1`, whose tree is not the
    // leftover being removed.
    let tmp = repo_on_task_branch();
    write_file(&tmp.path().join("pkg1/node_modules/keep.js"), "keep\n");
    write_file(&tmp.path().join("pkg[1]/node_modules/x.js"), "x\n");
    run_git(
        tmp.path(),
        &["add", "-f", "--", ":(literal)pkg1", ":(literal)pkg[1]"],
    );

    let output = run_guard(tmp.path(), "pre-commit");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    let removed = dry_run_printed_untrack(tmp.path(), &combined, "pkg[1]/node_modules");
    assert_eq!(
        removed,
        vec!["rm 'pkg[1]/node_modules/x.js'".to_string()],
        "the printed command must not reach pkg1/; got:\n{combined}"
    );
}

#[test]
fn cli_suggests_plain_rm_for_an_untracked_path_in_a_shared_root() {
    // `node_modules` holds pre-existing tracked entries, so the guard lists per
    // path; the untracked one is not in the index and `git rm --cached` on it
    // fails with "pathspec did not match any files".
    let tmp = repo_with_preexisting_tracked_node_modules();
    write_file(
        &tmp.path().join("node_modules/newdep/index.js"),
        "module.exports = 1;\n",
    );

    let output = run_guard(tmp.path(), "pre-publish");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    assert!(
        combined.contains("rm -- 'node_modules/newdep/index.js'"),
        "the untracked path must get a plain rm; got:\n{combined}"
    );
    assert!(
        !combined.contains("rm --cached -- 'node_modules/newdep/index.js'"),
        "an untracked path must never get git rm --cached; got:\n{combined}"
    );
}

#[test]
fn cli_never_deletes_an_allowlisted_file_the_mode_does_not_scan() {
    // `staged` never lists untracked files and the gates skip ignored ones
    // (issue #928), yet `rm -rf -- 'node_modules'` would still delete the
    // allowlisted `node_modules/p/keep.js` from disk.
    let tmp = repo_on_task_branch();
    write_file(
        &tmp.path().join(".amplihack-artifact-allowlist"),
        "node_modules/p/keep.js\n",
    );
    write_file(&tmp.path().join("node_modules/p/keep.js"), "keep\n");
    write_file(&tmp.path().join("node_modules/junk.js"), "junk\n");
    run_git(tmp.path(), &["add", "-f", "node_modules/junk.js"]);

    for ignored in [false, true] {
        if ignored {
            write_file(&tmp.path().join(".gitignore"), "node_modules/\n");
        }
        for mode in ["staged", "pre-commit", "pre-publish", "worktree"] {
            let output = run_guard(tmp.path(), mode);
            let combined = combined(&output);
            assert_eq!(output.status.code(), Some(1), "{mode}: {combined}");
            assert!(
                combined.contains("node_modules/junk.js"),
                "{mode}: the staged junk must be reported; got:\n{combined}"
            );
            for line in combined.lines().filter(|line| line.contains("rm ")) {
                assert!(
                    !line.contains("'node_modules'"),
                    "{mode} (ignored: {ignored}): no command may sweep the allowlisted file: {line}"
                );
            }
        }
    }
}

#[test]
fn cli_never_deletes_a_nested_repository_under_a_directory_root() {
    // `git ls-files --others` lists a nested repository as one entry and never
    // looks inside, so an allowlisted file in it is invisible to the guard.
    let tmp = repo_on_task_branch();
    write_file(
        &tmp.path().join(".amplihack-artifact-allowlist"),
        "node_modules/vend/keep.js\n",
    );
    run_git(tmp.path(), &["init", "-q", "node_modules/vend"]);
    write_file(&tmp.path().join("node_modules/vend/keep.js"), "keep\n");
    write_file(&tmp.path().join("node_modules/junk.js"), "junk\n");
    run_git(tmp.path(), &["add", "-f", "node_modules/junk.js"]);

    let output = run_guard(tmp.path(), "staged");
    let combined = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{combined}");
    for line in combined.lines().filter(|line| line.contains("rm ")) {
        assert!(
            !line.contains("'node_modules'"),
            "no command may sweep the nested repository: {line}"
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #1536, round 3: the caller's pathspec environment must not change the
// verdict. `--mode staged` finds manifests with `*/package.json`-style
// pathspecs, which only match across `/` under git's default rules. Magit and
// Emacs VC export `GIT_LITERAL_PATHSPECS=1`, and a hook passes it on.
// ---------------------------------------------------------------------------

const PATHSPEC_ENV: &[(&str, &str)] = &[
    ("GIT_LITERAL_PATHSPECS", "1"),
    ("GIT_GLOB_PATHSPECS", "1"),
    ("GIT_NOGLOB_PATHSPECS", "1"),
    ("GIT_ICASE_PATHSPECS", "1"),
];

fn run_guard_with_env(repo: &Path, mode: &str, env: (&str, &str)) -> std::process::Output {
    Command::new(bin())
        .args(["hygiene", "artifact-guard", "--mode", mode, "--repo"])
        .arg(repo)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env(env.0, env.1)
        .output()
        .expect("run artifact guard")
}

#[test]
fn cli_staged_coverage_app_passes_whatever_the_pathspec_environment() {
    let tmp = repo_on_task_branch();
    for (path, content) in [
        ("apps/coverage/package.json", "{\"name\":\"coverage\"}\n"),
        ("apps/coverage/tsconfig.json", "{}\n"),
        ("apps/coverage/src/i.ts", "export const x = 1;\n"),
    ] {
        write_file(&tmp.path().join(path), content);
    }
    run_git(tmp.path(), &["add", "apps"]);

    for env in PATHSPEC_ENV {
        for mode in ["staged", "pre-commit", "pre-publish"] {
            let output = run_guard_with_env(tmp.path(), mode, *env);
            let combined = combined(&output);
            assert_eq!(
                output.status.code(),
                Some(0),
                "staged apps/coverage source must pass {mode} with {}={}\n{combined}",
                env.0,
                env.1
            );
            assert!(
                !combined.contains("apps/coverage"),
                "{mode} with {}={} must not report apps/coverage; got:\n{combined}",
                env.0,
                env.1
            );
        }
    }
}

#[test]
fn cli_directory_root_listing_survives_an_inherited_pathspec_environment() {
    // A staged `node_modules` file gives a directory removal root, so the guard
    // lists the files under it with `--literal-pathspecs`. git refuses that
    // flag alongside an inherited GIT_GLOB_PATHSPECS, which must not turn a
    // plain block (exit 1) into a git failure (exit 2).
    let tmp = repo();
    write_file(
        &tmp.path().join("node_modules/leak/index.js"),
        "module.exports = 1;\n",
    );
    run_git(tmp.path(), &["add", "-f", "node_modules/leak/index.js"]);

    for env in PATHSPEC_ENV {
        for mode in ["staged", "pre-commit"] {
            let output = run_guard_with_env(tmp.path(), mode, *env);
            let combined = combined(&output);
            assert_eq!(
                output.status.code(),
                Some(1),
                "{mode} with {}={} must block, not fail\n{combined}",
                env.0,
                env.1
            );
            assert!(
                combined.contains("node_modules/leak/index.js"),
                "{mode} with {}={} must name the staged artifact; got:\n{combined}",
                env.0,
                env.1
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Issue #1528: run worktrees recorded as gitlinks.
//
// The refusal names the gitlink and prints the one command that removes it in
// every state a run leaves it in: `git rm --cached -f -- '<path>'`. It touches
// only the index. The CLI never prints `rm -rf`, or `git rm` without
// `--cached`, for a gitlink or for a root that holds a registered worktree.
// Every path printed inside a command is shell-quoted; a path with a control
// character gets no command at all. The guard stays report-only.
// ---------------------------------------------------------------------------

const GITLINK_RULE: &str = "run-worktree-gitlink";

fn hermetic(command: &mut Command) -> &mut Command {
    amplihack_git::scrub(command)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Artifact Guard Test")
        .env("GIT_AUTHOR_EMAIL", "artifact-guard@example.invalid")
        .env("GIT_COMMITTER_NAME", "Artifact Guard Test")
        .env("GIT_COMMITTER_EMAIL", "artifact-guard@example.invalid")
        .env_remove("AMPLIHACK_ARTIFACT_GUARD_BASELINE")
        .env_remove("AMPLIHACK_ARTIFACT_GUARD_PREEXISTING")
}

/// Hermetic git; returns stdout.
fn hgit(repo: &Path, args: &[&str]) -> String {
    let output = hermetic(amplihack_git::command().args(args).current_dir(repo))
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

/// Write one NUL-terminated `update-index -z --index-info` record.
fn hgit_index_info(repo: &Path, mode: &str, sha: &str, path: &str) {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = hermetic(
        amplihack_git::command()
            .args(["update-index", "-z", "--index-info"])
            .current_dir(repo),
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .expect("spawn git update-index");
    let mut record = format!("{mode} {sha} 0\t{path}").into_bytes();
    record.push(0);
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&record)
        .expect("write index-info");
    let output = child.wait_with_output().expect("wait git update-index");
    assert!(
        output.status.success(),
        "git update-index --index-info failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A main checkout on `main` with `worktrees/` ignored: the issue #1528 shape.
fn gitlink_repo_at(dir: &Path) {
    fs::create_dir_all(dir).expect("create repo dir");
    hgit(dir, &["init", "-q"]);
    hgit(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    write_file(&dir.join("README.md"), "# fixture\n");
    write_file(&dir.join(".gitignore"), "worktrees/\n");
    hgit(dir, &["add", "README.md", ".gitignore"]);
    hgit(dir, &["commit", "-qm", "initial"]);
}

fn gitlink_repo() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    gitlink_repo_at(tmp.path());
    tmp
}

/// Register `path` as a linked worktree on a new branch, leave it dirty (an
/// edit and an untracked file), and record it as a gitlink in the main index.
fn staged_dirty_run_worktree(repo: &Path, path: &str, branch: &str) {
    hgit(repo, &["worktree", "add", "-q", path, "-b", branch]);
    write_file(
        &repo.join(path).join("README.md"),
        "# uncommitted run work\n",
    );
    write_file(&repo.join(path).join("wip.txt"), "untracked run work\n");
    hgit(repo, &["add", "-f", "--", path]);
    assert!(
        index_gitlinks(repo).iter().any(|p| p == path),
        "fixture: {path} must be a gitlink in the index"
    );
}

fn index_gitlinks(repo: &Path) -> Vec<String> {
    hgit(repo, &["ls-files", "-s", "-z"])
        .split('\0')
        .filter_map(|record| {
            let (meta, path) = record.split_once('\t')?;
            meta.starts_with("160000 ").then(|| path.to_string())
        })
        .collect()
}

fn cached_raw(repo: &Path) -> String {
    hgit(repo, &["diff", "--cached", "--raw"])
}

fn canonical(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
        .display()
        .to_string()
}

fn guard(repo: &Path, args: &[&str], envs: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new(bin());
    hermetic(&mut command)
        .args(["hygiene", "artifact-guard"])
        .args(args)
        .arg("--repo")
        .arg(repo)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run artifact guard")
}

/// Run one printed remediation line exactly as an operator pasting it would.
fn run_printed_line(repo: &Path, line: &str) -> std::process::Output {
    let bin_dir = Path::new(bin()).parent().expect("binary dir");
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    hermetic(Command::new("bash").arg("-c").arg(line).current_dir(repo))
        .env("PATH", path)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("run printed line in bash")
}

/// The printed command lines (trimmed) that start with `prefix`.
fn printed_lines(output: &str, prefix: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with(prefix))
        .map(str::to_string)
        .collect()
}

fn assert_never_deletes(output: &str, root_or_path: &str) {
    for line in output.lines().map(str::trim) {
        let deletes = line.starts_with("rm -rf")
            || (line.starts_with("git rm") && !line.contains("--cached"));
        assert!(
            !(deletes && line.contains(root_or_path)),
            "the CLI must never print a deleting command for `{root_or_path}`; got line \
             `{line}` in:\n{output}"
        );
    }
}

#[test]
fn cli_blocks_run_worktree_gitlink_with_index_only_remediation() {
    // AC7.
    let tmp = gitlink_repo();
    staged_dirty_run_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    let before = cached_raw(tmp.path());

    let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    let text = combined(&output);

    assert_eq!(output.status.code(), Some(1), "{text}");
    for required in [
        "worktrees/feat/x",
        GITLINK_RULE,
        "git rm --cached -f -- 'worktrees/feat/x'",
        "never delete it",
        "commit from inside the PR worktree",
        "allowlist does not apply",
    ] {
        assert!(
            text.contains(required),
            "the refusal must print `{required}`; got:\n{text}"
        );
    }
    assert!(
        text.contains(&format!(
            "amplihack hygiene artifact-guard --repo '{}' --mode pre-commit",
            canonical(tmp.path())
        )),
        "the re-run line must shell-quote --repo; got:\n{text}"
    );
    assert_never_deletes(&text, "worktrees");
    assert!(
        !text.contains("git rm -r --cached -- 'worktrees'"),
        "a gitlink is never grouped under an artifact root; got:\n{text}"
    );

    assert_eq!(
        cached_raw(tmp.path()),
        before,
        "the guard is report-only: the index must be unchanged"
    );
    assert!(
        hgit(tmp.path(), &["worktree", "list", "--porcelain"])
            .contains(&format!("{}/worktrees/feat/x", canonical(tmp.path()))),
        "the run worktree must still be registered"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("worktrees/feat/x/README.md")).expect("read"),
        "# uncommitted run work\n",
        "the run worktree's uncommitted work must be intact"
    );
    assert!(tmp.path().join("worktrees/feat/x/wip.txt").exists());
}

#[test]
fn printed_gitlink_remediation_unstages_without_touching_the_worktree() {
    // V1: plain `git rm --cached` fails on a staged gitlink whose worktree has
    // moved on or is dirty, and `git restore --staged` leaves an in-HEAD
    // gitlink in place. The printed line must work in both states.
    for committed in [false, true] {
        let tmp = gitlink_repo();
        let path = "worktrees/feat/x";
        staged_dirty_run_worktree(tmp.path(), path, "feat/x");
        let wt = tmp.path().join(path);
        if committed {
            hgit(tmp.path(), &["commit", "-qm", "oops: gitlink"]);
        }
        // The run keeps working after the entry was recorded.
        hgit(&wt, &["add", "wip.txt"]);
        hgit(&wt, &["commit", "-qm", "run work"]);
        write_file(&wt.join("more.txt"), "more uncommitted work\n");
        let wt_head = hgit(&wt, &["rev-parse", "HEAD"]);

        let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
        let text = combined(&output);
        assert_eq!(output.status.code(), Some(1), "{text}");
        let lines = printed_lines(&text, "git rm --cached -f -- 'worktrees/feat/x'");
        assert_eq!(
            lines.len(),
            1,
            "exactly one gitlink remediation line (committed={committed}); got:\n{text}"
        );

        let ran = run_printed_line(tmp.path(), &lines[0]);
        assert!(
            ran.status.success(),
            "the printed line must succeed (committed={committed}): {}\n{}",
            lines[0],
            String::from_utf8_lossy(&ran.stderr)
        );
        assert!(
            !index_gitlinks(tmp.path()).contains(&path.to_string()),
            "the gitlink must be gone from the index (committed={committed})"
        );
        assert!(
            hgit(tmp.path(), &["worktree", "list", "--porcelain"]).contains("feat/x"),
            "the worktree must stay registered"
        );
        assert_eq!(hgit(&wt, &["rev-parse", "HEAD"]), wt_head);
        assert!(wt.join("more.txt").exists() && wt.join("wip.txt").exists());

        let rerun = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
        assert_eq!(
            rerun.status.code(),
            Some(0),
            "after the printed remedy the gate passes (committed={committed}):\n{}",
            combined(&rerun)
        );
    }
}

#[test]
fn cli_never_suggests_deleting_a_root_that_holds_registered_worktrees() {
    // A staged regular file under `worktrees/` next to a live, unstaged run
    // worktree. The library narrows the `worktrees` root, which holds the
    // registered worktree, to the violating path (#1536); that path is under
    // the reserved `worktrees/`, so it gets an index-only line and the
    // never-delete comment, and nothing is deleted.
    let tmp = gitlink_repo();
    hgit(
        tmp.path(),
        &[
            "worktree",
            "add",
            "-q",
            "worktrees/feat/pr-1530",
            "-b",
            "pr-1530",
        ],
    );
    write_file(&tmp.path().join("worktrees/old/notes.md"), "notes\n");
    hgit(tmp.path(), &["add", "-f", "worktrees/old/notes.md"]);

    let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("git rm -r --cached -- 'worktrees/old/notes.md'"),
        "the index-only line names the violating path; got:\n{text}"
    );
    assert!(
        printed_lines(&text, "git rm -r --cached -- 'worktrees'").is_empty(),
        "never untrack the whole `worktrees` root; got:\n{text}"
    );
    assert!(
        text.contains(&format!(
            "# never delete 'worktrees/old/notes.md': {NEVER_DELETE_RESERVED}"
        )),
        "the deleting line becomes a warning comment; got:\n{text}"
    );
    assert_never_deletes(&text, "worktrees");

    // Not special-cased to `worktrees/`: any root holding a registered worktree.
    let tmp = gitlink_repo();
    hgit(
        tmp.path(),
        &["worktree", "add", "-q", "build/review", "-b", "review"],
    );
    write_file(&tmp.path().join("build/out.txt"), "output\n");
    hgit(tmp.path(), &["add", "build/out.txt"]);

    let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    // `build` holds the registered `build/review`, so no root covering it is
    // printed (#1536 narrows it, and `build` is a generic name, so
    // `build/out.txt` is only reviewed); the worktree itself, untracked here
    // because `build/` is not ignored, gets the never-delete comment.
    assert!(
        text.contains(&format!(
            "# never delete 'build/review': {NEVER_DELETE_REGISTERED}"
        )),
        "a registered worktree is never deleted; got:\n{text}"
    );
    assert!(
        printed_lines(&text, "rm -rf -- 'build' ").is_empty()
            && printed_lines(&text, "git rm -r --cached -- 'build' ").is_empty(),
        "a root holding a registered worktree is never removed whole; got:\n{text}"
    );
    assert_never_deletes(&text, "'build'");
    assert_never_deletes(&text, "build/review");
    assert!(tmp.path().join("build/review/README.md").exists());
}

#[test]
fn forced_add_from_the_main_checkout_is_the_only_way_to_stage_a_run_worktree() {
    // D5: the #1528 reproduction. In a main checkout that ignores
    // `worktrees/`, `git add -A` stages nothing of a run worktree and an
    // explicit pathspec is refused as ignored. Only `-f` records the 160000
    // gitlink, and only the run-worktree-gitlink rule refuses it after staging.
    let tmp = gitlink_repo();
    let repo = tmp.path();
    hgit(
        repo,
        &["worktree", "add", "-q", "worktrees/feat/x", "-b", "feat/x"],
    );
    write_file(&repo.join("worktrees/feat/x/fix.rs"), "// the PR's work\n");
    // An aborted worktree copy with no `.git`, as in the 134-file case.
    write_file(&repo.join("worktrees/aborted/notes.md"), "stale copy\n");

    hgit(repo, &["add", "-A"]);
    assert!(
        hgit(repo, &["diff", "--cached", "--name-only"]).is_empty(),
        "`git add -A` must stage nothing under an ignored worktrees/"
    );

    let explicit = hermetic(amplihack_git::command().current_dir(repo))
        .args(["add", "--", "worktrees/feat/x"])
        .output()
        .expect("run git add");
    assert!(
        !explicit.status.success(),
        "an explicit pathspec under an ignored worktrees/ must be refused"
    );
    assert!(
        String::from_utf8_lossy(&explicit.stderr).contains("ignored"),
        "git must say the path is ignored; got:\n{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    assert!(hgit(repo, &["diff", "--cached", "--name-only"]).is_empty());

    // Guarding before staging (the pre-#1528 order) sees a clean index.
    let before = guard(repo, &["--mode", "pre-commit"], &[]);
    assert_eq!(before.status.code(), Some(0), "{}", combined(&before));

    hgit(repo, &["add", "-A", "-f"]);
    assert_eq!(
        index_gitlinks(repo),
        vec!["worktrees/feat/x".to_string()],
        "the forced add records the linked worktree as a 160000 gitlink"
    );
    assert!(
        hgit(repo, &["diff", "--cached", "--name-only"])
            .lines()
            .any(|line| line == "worktrees/aborted/notes.md"),
        "the forced add also sweeps up the aborted copy's files"
    );

    // Guarding after staging is what catches it.
    let after = guard(repo, &["--mode", "pre-commit"], &[]);
    let text = combined(&after);
    assert_eq!(after.status.code(), Some(1), "{text}");
    assert!(text.contains("run-worktree-gitlink"), "{text}");
    assert!(
        text.contains("git rm --cached -f -- 'worktrees/feat/x'"),
        "the remedy is index-only; got:\n{text}"
    );
    assert_never_deletes(&text, "worktrees");
    assert!(repo.join("worktrees/feat/x/fix.rs").exists());
}

#[test]
fn gitlink_remediation_with_quote_in_path_round_trips_through_bash() {
    // SAC5 / SEC-7.
    let tmp = gitlink_repo();
    let path = "worktrees/a'b";
    staged_dirty_run_worktree(tmp.path(), path, "ab");

    let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    let lines = printed_lines(&text, r"git rm --cached -f -- 'worktrees/a'\''b'");
    assert_eq!(
        lines.len(),
        1,
        "the quote must be written as '\\'' inside single quotes; got:\n{text}"
    );

    let ran = run_printed_line(tmp.path(), &lines[0]);
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(!index_gitlinks(tmp.path()).contains(&path.to_string()));
    assert!(tmp.path().join(path).join("wip.txt").exists());
    assert_eq!(
        guard(tmp.path(), &["--mode", "pre-commit"], &[])
            .status
            .code(),
        Some(0)
    );
}

#[test]
fn run_worktree_gitlink_blocks_despite_baseline_and_preexisting_env() {
    // SAC6: committed on `main` before the task branch, covered by an exact
    // allowlist entry, under every way of selecting `report`.
    let tmp = gitlink_repo();
    hgit(
        tmp.path(),
        &["worktree", "add", "-q", "worktrees/feat/x", "-b", "feat/x"],
    );
    hgit(tmp.path(), &["add", "-f", "--", "worktrees/feat/x"]);
    hgit(tmp.path(), &["commit", "-qm", "history: gitlink"]);
    hgit(tmp.path(), &["checkout", "-q", "-b", "task/unrelated"]);
    write_file(&tmp.path().join("package.json"), "{}\n");
    hgit(tmp.path(), &["add", "package.json"]);
    hgit(tmp.path(), &["commit", "-qm", "unrelated"]);
    write_file(
        &tmp.path().join(".amplihack-artifact-allowlist"),
        "worktrees/feat/x\n",
    );

    let env = [
        ("AMPLIHACK_ARTIFACT_GUARD_BASELINE", "main"),
        ("AMPLIHACK_ARTIFACT_GUARD_PREEXISTING", "report"),
    ];
    for (args, envs) in [
        (vec!["--mode", "pre-publish"], &env[..]),
        (vec!["--mode", "pre-commit"], &env[..]),
        (
            vec![
                "--mode",
                "pre-publish",
                "--preexisting",
                "report",
                "--baseline",
                "main",
            ],
            &[][..],
        ),
    ] {
        let output = guard(tmp.path(), &args, envs);
        let text = combined(&output);
        assert_eq!(
            output.status.code(),
            Some(1),
            "args {args:?} env {envs:?}:\n{text}"
        );
        assert!(
            text.contains(GITLINK_RULE)
                && text.contains("git rm --cached -f -- 'worktrees/feat/x'"),
            "args {args:?} env {envs:?}:\n{text}"
        );
    }
}

#[test]
fn cli_worktree_and_staged_modes_report_a_committed_run_worktree_gitlink() {
    // The rule runs in every mode. Without it, worktree mode would print the
    // root-grouped `git rm -r --cached -- 'worktrees'`, which has no `-f` and
    // can fail on the gitlink.
    let tmp = gitlink_repo();
    staged_dirty_run_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    hgit(tmp.path(), &["commit", "-qm", "oops: gitlink"]);

    for mode in ["worktree", "staged", "all"] {
        let output = guard(tmp.path(), &["--mode", mode], &[]);
        let text = combined(&output);
        assert_eq!(output.status.code(), Some(1), "mode {mode}:\n{text}");
        assert!(
            text.contains(GITLINK_RULE)
                && text.contains("git rm --cached -f -- 'worktrees/feat/x'"),
            "mode {mode}:\n{text}"
        );
        assert!(
            !text.contains("git rm -r --cached -- 'worktrees'"),
            "mode {mode}:\n{text}"
        );
        assert_never_deletes(&text, "worktrees");
    }
}

#[test]
fn control_character_path_prints_no_command() {
    // SEC-7: a path with a newline is printed escaped and gets no command, so
    // a pasted line can never be split into two shell commands.
    let tmp = gitlink_repo();
    let sha = hgit(tmp.path(), &["rev-parse", "HEAD"]);
    hgit_index_info(tmp.path(), "160000", sha.trim(), "worktrees/a\nb");

    let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains(GITLINK_RULE),
        "the path is still reported; got:\n{text}"
    );
    assert!(
        text.contains(r"worktrees/a\nb"),
        "the path must be printed escaped; got:\n{text}"
    );
    assert!(
        !text.contains("worktrees/a\nb"),
        "the raw newline must never be printed; got:\n{text}"
    );
    for line in text.lines().map(str::trim) {
        assert!(
            !((line.starts_with("git rm") || line.starts_with("rm "))
                && line.contains("worktrees/a")),
            "no command may be printed for a control-character path; got `{line}`"
        );
    }
}

#[test]
fn control_character_repo_path_prints_no_rerun_command() {
    // A tab stays on one porcelain line, so the scan reports (exit 1). A
    // newline splits the main checkout's own `worktree` record before `HEAD`,
    // so the worktree list is ambiguous and the guard refuses (exit 2,
    // SEC-V6). Neither prints a re-run line.
    for (name, code) in [("re\tpo", 1), ("re\npo", 2)] {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path().join(name);
        gitlink_repo_at(&repo);
        write_file(&repo.join("dist/plugin.js"), "generated\n");
        hgit(&repo, &["add", "-f", "dist/plugin.js"]);

        let output = guard(&repo, &["--mode", "pre-commit"], &[]);
        let text = combined(&output);
        assert_eq!(output.status.code(), Some(code), "{name:?}: {text}");
        assert!(
            !text.contains("artifact-guard --repo"),
            "{name:?}: no re-run line may embed a repository path with a control character; got:\n{text}"
        );
    }
}

#[test]
fn printed_repo_path_with_a_space_round_trips_through_bash() {
    // SEC-7: the re-run line quotes --repo, so it runs as printed.
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("my repo's copy");
    gitlink_repo_at(&repo);
    write_file(&repo.join("dist/plugin.js"), "generated\n");
    hgit(&repo, &["add", "-f", "dist/plugin.js"]);

    let output = guard(&repo, &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    let rerun = printed_lines(&text, "amplihack hygiene artifact-guard --repo ");
    assert_eq!(rerun.len(), 1, "one re-run line; got:\n{text}");

    let ran = run_printed_line(tmp.path(), &rerun[0]);
    assert_eq!(
        ran.status.code(),
        Some(1),
        "the pasted re-run line must scan the same repository and refuse again \
         (exit 2 would mean the path was split):\n{}",
        combined(&ran)
    );
}

#[test]
fn paths_with_quotes_get_review_not_a_removal_command() {
    // SEC-7 on the removal lines. This PR first printed such paths quoted as
    // `'\''`; #1536 (merged from origin/main) decided a path holding `'` gets
    // no removal command at all, only review, and that policy is kept for
    // every rule but run-worktree-gitlink, whose index-only line still quotes
    // (`gitlink_remediation_with_quote_in_path_round_trips_through_bash`). What
    // must hold either way: no printed command splits on the quote, and the
    // path is shown escaped.
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path();
    gitlink_repo_at(repo);
    write_file(&repo.join("vendor/it's/node_modules/a/index.js"), "a\n");
    hgit(repo, &["add", "-f", "vendor"]);
    hgit(repo, &["commit", "-qm", "history: vendored tree"]);
    hgit(repo, &["checkout", "-q", "-b", "task/dep"]);
    write_file(&repo.join("vendor/it's/node_modules/b/index.js"), "b\n");
    hgit(repo, &["add", "-f", "vendor/it's/node_modules/b/index.js"]);
    hgit(repo, &["commit", "-qm", "add one dep"]);

    let output = guard(repo, &["--mode", "pre-publish", "--baseline", "main"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    for line in text.lines().map(str::trim) {
        let command = line.starts_with("git ") || line.starts_with("rm ");
        assert!(
            !(command && line.contains("it's")),
            "no removal command may embed the quoted path; got `{line}` in:\n{text}"
        );
    }
    assert!(
        text.contains(r#""vendor/it's/node_modules/b/index.js""#),
        "the introduced path is shown escaped; got:\n{text}"
    );
    assert!(
        cleanup_line(&text).is_none(),
        "no clean-up command for the pre-existing tree; got:\n{text}"
    );
    assert!(text.contains("needs manual review"), "{text}");
    let tracked = hgit(repo, &["ls-files"]);
    assert!(
        tracked.contains("node_modules/a/index.js") && tracked.contains("node_modules/b/index.js"),
        "nothing ran; tracked:\n{tracked}"
    );
}

// ---------------------------------------------------------------------------
// Issue #1528: the worktree list is read fail-loud (D6, D13, SEC-V6).
//
// `registered_worktrees` decides whether a root may be printed with `rm -rf`,
// so a list the guard cannot trust ends the run with exit 2, no report and no
// commands. `amplihack_git::command()` is a plain git `Command`, so a `git`
// shim first on the guard's PATH intercepts exactly the `worktree list` call
// and hands every other call to the real git.
// ---------------------------------------------------------------------------

const ALL_MODES: [&str; 5] = ["pre-commit", "pre-publish", "all", "staged", "worktree"];

const SCAN_ERROR_HINT: &str =
    "Fix the repository path, Git state, mode, allowlist, or baseline and rerun the guard.";

fn real_git() -> std::path::PathBuf {
    let path = std::env::var_os("PATH").expect("PATH is set");
    let git = std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH");
    assert!(
        !git.to_string_lossy().contains('\''),
        "the real git path must be embeddable in a single-quoted shim: {}",
        git.display()
    );
    git
}

/// Write `<dir>/git`: on `... worktree list ...` run `on_list` (a bash
/// fragment), otherwise exec the real git. Returns a PATH with the shim first.
#[cfg(unix)]
fn git_shim(dir: &Path, on_list: &str) -> String {
    use std::os::unix::fs::PermissionsExt;

    let shim = dir.join("git");
    write_file(
        &shim,
        &format!(
            "#!/usr/bin/env bash\n\
             prev=''\n\
             for arg in \"$@\"; do\n\
             \x20 if [ \"$prev\" = worktree ] && [ \"$arg\" = list ]; then\n\
             \x20   {on_list}\n\
             \x20 fi\n\
             \x20 prev=\"$arg\"\n\
             done\n\
             exec '{}' \"$@\"\n",
            real_git().display()
        ),
    );
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("chmod shim");
    format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// A repository where every mode would otherwise print commands: a staged
/// plugin bundle (`rm -rf -- 'dist'` today) and a staged run-worktree gitlink.
fn repo_that_would_print_commands() -> TempDir {
    let tmp = gitlink_repo();
    staged_dirty_run_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    write_file(&tmp.path().join("dist/plugin.js"), "generated\n");
    hgit(tmp.path(), &["add", "-f", "dist/plugin.js"]);
    let baseline = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    assert!(
        combined(&baseline).contains("git rm"),
        "fixture: without a failing worktree list the guard prints commands:\n{}",
        combined(&baseline)
    );
    tmp
}

/// Exit 2, the error line, and nothing that reads as a report or a command.
fn assert_exit_2_without_report(output: &std::process::Output, expected: &str, context: &str) {
    let text = combined(output);
    assert_eq!(output.status.code(), Some(2), "{context}:\n{text}");
    assert!(
        text.contains(&format!("Artifact Guard configuration error: {expected}")),
        "{context}: expected `{expected}`; got:\n{text}"
    );
    assert!(
        text.contains(SCAN_ERROR_HINT),
        "{context}: a scan-time error is followed by the hint line; got:\n{text}"
    );
    for forbidden in [
        "rm -rf",
        "git rm",
        "Artifact Guard blocked",
        "Exact commands",
        "Artifact Guard clean",
        "pre-existing prohibited artifact",
    ] {
        assert!(
            !text.contains(forbidden),
            "{context}: exit 2 prints no report and no commands, found `{forbidden}`:\n{text}"
        );
    }
}

#[cfg(unix)]
#[test]
fn worktree_list_failure_exits_2_in_every_mode_and_never_prints_rm_rf() {
    // AC4 / D13: a failed `git worktree list` is never read as an empty list.
    let tmp = repo_that_would_print_commands();
    let shims = TempDir::new().expect("shim dir");
    let path = git_shim(
        shims.path(),
        "echo 'fatal: shim refused worktree list' >&2; exit 128",
    );
    let before = cached_raw(tmp.path());

    for mode in ALL_MODES {
        let output = guard(tmp.path(), &["--mode", mode], &[("PATH", &path)]);
        assert_exit_2_without_report(
            &output,
            "artifact guard git command failed: git worktree list --porcelain",
            &format!("mode {mode}"),
        );
        let text = combined(&output);
        assert!(
            text.contains("128") && text.contains("shim refused worktree list"),
            "mode {mode}: the error carries git's exit code and stderr; got:\n{text}"
        );
    }
    assert_eq!(cached_raw(tmp.path()), before, "the guard writes nothing");
    assert!(tmp.path().join("worktrees/feat/x/wip.txt").exists());
}

#[cfg(unix)]
#[test]
fn untrustworthy_worktree_list_output_exits_2_with_a_fixed_payload_in_every_mode() {
    // D6 / SEC-V6: git exited 0 but its output cannot be trusted.
    let tmp = repo_that_would_print_commands();
    let top = canonical(tmp.path());
    let sha = hgit(tmp.path(), &["rev-parse", "HEAD"]).trim().to_string();
    let cases = [
        ("no worktree records", String::new()),
        (
            "a worktree record is not an absolute path",
            format!(
                "worktree {top}\nHEAD {sha}\nbranch refs/heads/main\n\n\
                 worktree worktrees/feat/x\nHEAD {sha}\nbranch refs/heads/feat/x\n\n"
            ),
        ),
        (
            "ambiguous worktree records",
            format!(
                "worktree {top}\nHEAD {sha}\nbranch refs/heads/main\n\n\
                 worktree {top}/worktrees/a\nworktree {top}/worktrees/feat/x\nHEAD {sha}\n\
                 branch refs/heads/feat/x\n\n"
            ),
        ),
    ];

    for (payload, porcelain) in cases {
        let shims = TempDir::new().expect("shim dir");
        let canned = shims.path().join("porcelain.txt");
        write_file(&canned, &porcelain);
        let path = git_shim(shims.path(), &format!("cat '{}'; exit 0", canned.display()));
        for mode in ALL_MODES {
            let output = guard(tmp.path(), &["--mode", mode], &[("PATH", &path)]);
            assert_exit_2_without_report(
                &output,
                &format!("artifact guard could not list worktrees: {payload}"),
                &format!("{payload} / mode {mode}"),
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn forged_worktree_record_from_a_newline_path_exits_2_in_every_mode() {
    // SEC-V6 with real git: a worktree path holding `\nworktree <abs>` prints
    // as two records. The guard refuses rather than guess which is real.
    let tmp = repo_that_would_print_commands();
    let top = canonical(tmp.path());
    let forged = format!("{top}/x\nworktree {top}/worktrees/forged");
    hgit(
        tmp.path(),
        &["worktree", "add", "-q", &forged, "-b", "forged"],
    );

    for mode in ALL_MODES {
        let output = guard(tmp.path(), &["--mode", mode], &[]);
        assert_exit_2_without_report(
            &output,
            "artifact guard could not list worktrees: ambiguous worktree records",
            &format!("mode {mode}"),
        );
    }
}

#[cfg(unix)]
#[test]
fn newline_path_without_a_forged_record_exits_2_in_every_mode() {
    // SEC-V6 with real git: `worktrees/a<LF>b` prints `worktree …/a`, a stray
    // `b`, then `HEAD`. The stray line before `HEAD` is refused, not skipped.
    let tmp = repo_that_would_print_commands();
    let top = canonical(tmp.path());
    let path = format!("{top}/worktrees/a\nb");
    hgit(
        tmp.path(),
        &["worktree", "add", "-q", &path, "-b", "newline"],
    );

    for mode in ALL_MODES {
        let output = guard(tmp.path(), &["--mode", mode], &[]);
        assert_exit_2_without_report(
            &output,
            "artifact guard could not list worktrees: ambiguous worktree records",
            &format!("mode {mode}"),
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #1528: unsafe characters (SEC-O3) — escaped everywhere, never put in a
// command.
// ---------------------------------------------------------------------------

/// Write one NUL-terminated `update-index -z --index-info` record whose path
/// is raw bytes (not necessarily UTF-8).
fn hgit_index_info_bytes(repo: &Path, mode: &str, sha: &str, path: &[u8]) {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = hermetic(
        amplihack_git::command()
            .args(["update-index", "-z", "--index-info"])
            .current_dir(repo),
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .expect("spawn git update-index");
    let mut record = format!("{mode} {sha} 0\t").into_bytes();
    record.extend_from_slice(path);
    record.push(0);
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&record)
        .expect("write index-info");
    let output = child.wait_with_output().expect("wait git update-index");
    assert!(
        output.status.success(),
        "git update-index --index-info failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `(raw path bytes, raw char that must never be printed, escaped form)`.
fn unsafe_gitlink_paths() -> Vec<(Vec<u8>, String, &'static str)> {
    let utf8 = |path: &str, raw: &str, escaped: &'static str| {
        (path.as_bytes().to_vec(), raw.to_string(), escaped)
    };
    vec![
        utf8("worktrees/n\nb", "\n", r"worktrees/n\nb"),
        utf8("worktrees/t\tb", "\t", r"worktrees/t\tb"),
        utf8("worktrees/r\rb", "\r", r"worktrees/r\rb"),
        utf8(
            "worktrees/e\u{1b}[31mb",
            "\u{1b}",
            r"worktrees/e\u{1b}[31mb",
        ),
        utf8("worktrees/d\u{7f}b", "\u{7f}", r"worktrees/d\u{7f}b"),
        utf8("worktrees/c\u{85}b", "\u{85}", r"worktrees/c\u{85}b"),
        utf8("worktrees/o\u{202e}b", "\u{202e}", r"worktrees/o\u{202e}b"),
        utf8("worktrees/i\u{2066}b", "\u{2066}", r"worktrees/i\u{2066}b"),
        utf8("worktrees/m\u{200f}b", "\u{200f}", r"worktrees/m\u{200f}b"),
        utf8("worktrees/l\u{200e}b", "\u{200e}", r"worktrees/l\u{200e}b"),
        utf8("worktrees/a\u{61c}b", "\u{61c}", r"worktrees/a\u{61c}b"),
        (
            b"worktrees/f\xffb".to_vec(),
            "\u{fffd}".to_string(),
            r"worktrees/f\u{fffd}b",
        ),
    ]
}

#[test]
fn unsafe_character_paths_are_escaped_and_get_a_comment_instead_of_a_command() {
    // SEC-O3: control, bidirectional/format and replacement characters. Every
    // gitlink is still reported and blocks; none gets a command.
    let tmp = gitlink_repo();
    let sha = hgit(tmp.path(), &["rev-parse", "HEAD"]).trim().to_string();
    let paths = unsafe_gitlink_paths();
    for (raw, _, _) in &paths {
        hgit_index_info_bytes(tmp.path(), "160000", &sha, raw);
    }

    let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains(&format!(
            "Artifact Guard blocked {} prohibited artifact path(s)",
            paths.len()
        )),
        "every unsafe gitlink is still reported; got:\n{text}"
    );
    for (_, raw_char, escaped) in &paths {
        if raw_char != "\n" {
            assert!(
                !text.contains(raw_char.as_str()),
                "the raw character {raw_char:?} must never be printed; got:\n{text}"
            );
        }
        assert!(
            text.lines()
                .any(|line| line.contains(escaped) && line.contains(GITLINK_RULE)),
            "the table row must show `{escaped}`; got:\n{text}"
        );
        let comment = format!(
            "# no command for {escaped}: the path contains an unsafe character; \
             inspect it and fix it by hand"
        );
        assert_eq!(
            printed_lines(&text, &comment).len(),
            1,
            "exactly one `{comment}` line; got:\n{text}"
        );
    }
    assert!(
        !text.contains("worktrees/n\nb"),
        "the raw newline path must never be printed; got:\n{text}"
    );
    for line in text.lines().map(str::trim) {
        assert!(
            !(line.starts_with("git rm") || line.starts_with("rm ")),
            "no command may be printed when every path is unsafe; got `{line}` in:\n{text}"
        );
    }
}

#[test]
fn unsafe_repository_paths_are_escaped_and_get_no_rerun_line() {
    // SEC-O3 on `--repo`: the header and the clean line print it escaped, and
    // no `artifact-guard --repo` line is ever built from it.
    for (dir, raw_char, escaped) in [
        ("re\u{202e}po", "\u{202e}", r"re\u{202e}po"),
        ("re\u{1b}po", "\u{1b}", r"re\u{1b}po"),
        ("re\tpo", "\t", r"re\tpo"),
    ] {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path().join(dir);
        gitlink_repo_at(&repo);

        let clean = guard(&repo, &["--mode", "pre-commit"], &[]);
        let stdout = String::from_utf8_lossy(&clean.stdout).into_owned();
        assert_eq!(
            clean.status.code(),
            Some(0),
            "{dir:?}: {}",
            combined(&clean)
        );
        assert!(
            stdout.starts_with("Artifact Guard clean: ") && stdout.contains(escaped),
            "{dir:?}: the clean line prints the repository escaped; got:\n{stdout}"
        );
        assert!(
            !combined(&clean).contains(raw_char),
            "{dir:?}: the raw character must never be printed; got:\n{}",
            combined(&clean)
        );

        write_file(&repo.join("dist/plugin.js"), "generated\n");
        hgit(&repo, &["add", "-f", "dist/plugin.js"]);
        let blocked = guard(&repo, &["--mode", "pre-commit"], &[]);
        let text = combined(&blocked);
        assert_eq!(blocked.status.code(), Some(1), "{dir:?}:\n{text}");
        assert!(
            text.contains(&format!("in {}", canonical(tmp.path())))
                && text.contains(escaped)
                && !text.contains(raw_char),
            "{dir:?}: the header prints the repository escaped; got:\n{text}"
        );
        assert!(
            !text.contains("artifact-guard --repo"),
            "{dir:?}: no re-run line for an unsafe repository path; got:\n{text}"
        );
    }
}

#[test]
fn configuration_errors_print_untrusted_text_escaped() {
    // SEC-O6: an error line can carry Git's stderr, repository content or the
    // caller's own value. It is escaped by the same rule as every path. Errors
    // raised before the scan print no hint line.
    let tmp = gitlink_repo();

    let output = guard(tmp.path(), &["--mode", "pre\u{1b}[2Jcommit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(
        text.contains(
            r"Artifact Guard configuration error: artifact guard invalid mode: pre\u{1b}[2Jcommit"
        ),
        "bad --mode is escaped; got:\n{text}"
    );
    assert!(
        !text.contains('\u{1b}') && !text.contains(SCAN_ERROR_HINT),
        "{text}"
    );

    let output = guard(
        tmp.path(),
        &["--mode", "pre-commit"],
        &[("AMPLIHACK_ARTIFACT_GUARD_PREEXISTING", "rep\u{202e}ort")],
    );
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(
        text.contains(r"artifact guard invalid pre-existing policy: rep\u{202e}ort"),
        "bad --preexisting is escaped; got:\n{text}"
    );
    assert!(
        !text.contains('\u{202e}') && !text.contains(SCAN_ERROR_HINT),
        "{text}"
    );

    // Repository content: an allowlist entry with an escape sequence.
    write_file(
        &tmp.path().join(".amplihack-artifact-allowlist"),
        "/tmp/a\u{1b}[2Jb\n",
    );
    let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(
        text.contains(r"/tmp/a\u{1b}[2Jb") && !text.contains('\u{1b}'),
        "an allowlist error is escaped; got:\n{text}"
    );
    assert!(text.contains(SCAN_ERROR_HINT), "{text}");
    fs::remove_file(tmp.path().join(".amplihack-artifact-allowlist")).expect("rm allowlist");

    // Git's stderr: `git -C <repo>` names the missing directory.
    let missing = tmp.path().join("gone\tdir");
    let output = guard(&missing, &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(
        text.contains(r"gone\tdir") && !text.contains("gone\tdir"),
        "git's stderr is escaped; got:\n{text}"
    );
}

#[cfg(unix)]
#[test]
fn unreadable_current_directory_is_a_configuration_error_without_a_hint() {
    // The error is raised before the scan: one line, no hint.
    let tmp = TempDir::new().expect("tempdir");
    let doomed = tmp.path().join("doomed");
    fs::create_dir_all(&doomed).expect("mkdir");
    let output = hermetic(
        Command::new("bash")
            .arg("-c")
            .arg(
                "cd \"$1\" && rmdir \"$1\" && exec \"$2\" hygiene artifact-guard --mode pre-commit",
            )
            .arg("bash")
            .arg(&doomed)
            .arg(bin()),
    )
    .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
    .output()
    .expect("run guard from a deleted directory");
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(
        text.contains("Artifact Guard configuration error: resolve current directory:"),
        "{text}"
    );
    assert!(!text.contains(SCAN_ERROR_HINT), "{text}");
}

// ---------------------------------------------------------------------------
// Issue #1528: roots that are never deleted (SEC-O4) and gitlink ordering.
// ---------------------------------------------------------------------------

const NEVER_DELETE_REGISTERED: &str = "it holds registered worktree(s) — see git worktree list";
const NEVER_DELETE_RESERVED: &str =
    "it is reserved for run worktrees — inspect it and remove leftovers by hand";

#[test]
fn roots_under_worktrees_are_never_deleted_even_when_nothing_is_registered() {
    // SEC-O4: a directory under `worktrees/` whose registration was removed,
    // or never completed, can still hold a run's uncommitted work. The first
    // component is compared ignoring ASCII case. Each case gets its own
    // repository so `Worktrees/` cannot collide with `worktrees/` on a
    // case-insensitive filesystem.
    for (file, root) in [
        ("worktrees/old/notes.md", "worktrees"),
        (
            "Worktrees/x/node_modules/a/index.js",
            "Worktrees/x/node_modules",
        ),
    ] {
        let tmp = gitlink_repo();
        write_file(&tmp.path().join(file), "leftover\n");
        hgit(tmp.path(), &["add", "-f", "--", file]);

        let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
        let text = combined(&output);
        assert_eq!(output.status.code(), Some(1), "{root}:\n{text}");
        assert_eq!(
            printed_lines(&text, &format!("git rm -r --cached -- '{root}'")).len(),
            1,
            "the index-only line for `{root}` stays; got:\n{text}"
        );
        assert_eq!(
            printed_lines(
                &text,
                &format!("# never delete '{root}': {NEVER_DELETE_RESERVED}")
            )
            .len(),
            1,
            "`{root}` gets the reserved-root comment; got:\n{text}"
        );
        assert!(
            !text.contains("rm -rf"),
            "no `rm -rf` for a root under worktrees/ (any case); got:\n{text}"
        );
        assert!(
            !text.contains(NEVER_DELETE_REGISTERED),
            "nothing is registered here; got:\n{text}"
        );
    }
}

// The registered-before-reserved precedence of the never-delete comment is
// tested by `never_delete_reports_a_registered_worktree_before_the_reserved_dir`
// in crates/amplihack-cli/src/commands/hygiene/artifact_guard.rs. Since #1536
// the library narrows a root that holds a registered worktree to the violating
// path, so a scan no longer hands the CLI such a root; see
// `cli_never_suggests_deleting_a_root_that_holds_registered_worktrees`.

const GITLINK_BULLET: &str = "  - A run worktree is a live checkout recorded as a gitlink: drop it \
from the index only, never delete it, and commit from inside the PR worktree. The allowlist does \
not apply to run-worktree-gitlink.";

const GITLINK_LINE_SUFFIX: &str =
    "   # index only: keeps the worktree and its files; -f only overrides git's up-to-date check";

#[test]
fn gitlinks_lead_the_table_and_the_commands_and_add_the_fourth_bullet() {
    // D8 / D9: gitlink rows and lines come first; the fourth bullet appears
    // only when a gitlink blocks.
    let tmp = gitlink_repo();
    staged_dirty_run_worktree(tmp.path(), "worktrees/feat/x", "feat/x");
    write_file(&tmp.path().join("dist/plugin.js"), "generated\n");
    write_file(&tmp.path().join("node_modules/a/index.js"), "a\n");
    hgit(
        tmp.path(),
        &["add", "-f", "dist/plugin.js", "node_modules/a/index.js"],
    );

    let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    let lines: Vec<&str> = text.lines().collect();
    let at = |what: &str, predicate: fn(&str) -> bool| {
        lines
            .iter()
            .position(|line| predicate(line))
            .unwrap_or_else(|| panic!("missing {what}; got:\n{text}"))
    };

    let gitlink_row = at("gitlink row", |l| {
        l.contains(GITLINK_RULE) && l.contains("worktrees/feat/x")
    });
    let other_row = at("other row", |l| {
        l.contains("plugin-bundle") || l.contains("node-modules")
    });
    assert!(gitlink_row < other_row, "gitlink rows come first:\n{text}");

    assert!(
        lines.contains(&GITLINK_BULLET),
        "the fourth bullet, verbatim; got:\n{text}"
    );

    let gitlink_line = at("gitlink command", |l| {
        l.trim_start()
            .starts_with("git rm --cached -f -- 'worktrees/feat/x'")
    });
    assert_eq!(
        lines[gitlink_line].trim(),
        format!("git rm --cached -f -- 'worktrees/feat/x'{GITLINK_LINE_SUFFIX}"),
        "the gitlink line, verbatim"
    );
    let root_line = at("root-grouped command", |l| {
        l.trim_start().starts_with("git rm -r --cached --")
    });
    assert!(
        gitlink_line < root_line,
        "gitlink lines are printed before root-grouped commands:\n{text}"
    );

    let detail = at("detail header", |l| l == "First violation detail:");
    assert_eq!(
        lines.get(detail + 1).copied(),
        Some(
            "  worktrees/feat/x: run worktree recorded as a gitlink; drop the entry from the \
             index only (git rm --cached -f keeps the worktree and its files) and commit from \
             inside the PR worktree; cannot be allowlisted"
        ),
        "the first violation detail is the gitlink; got:\n{text}"
    );

    // No gitlink: no fourth bullet.
    let tmp = gitlink_repo();
    write_file(&tmp.path().join("dist/plugin.js"), "generated\n");
    hgit(tmp.path(), &["add", "-f", "dist/plugin.js"]);
    let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        !text.contains("A run worktree is a live checkout"),
        "the fourth bullet needs a blocking gitlink; got:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// Issue #1528: paths Git would read as a pathspec (SEC-O7) and `.gitignore`
// specials in the clean-up line (SEC-O5).
// ---------------------------------------------------------------------------

#[test]
fn gitlink_with_a_glob_in_its_path_is_removed_alone() {
    // SEC-O7: `git rm --cached -f -- 'wt*'` would also untrack `wtA` and
    // `wtB/deep/file.js`. The printed line runs `git --literal-pathspecs`.
    let tmp = gitlink_repo();
    write_file(&tmp.path().join("wtA"), "keep\n");
    write_file(&tmp.path().join("wtB/deep/file.js"), "keep\n");
    hgit(tmp.path(), &["add", "wtA", "wtB/deep/file.js"]);
    hgit(tmp.path(), &["commit", "-qm", "siblings"]);
    hgit(tmp.path(), &["worktree", "add", "-q", "wt*", "-b", "star"]);
    hgit(tmp.path(), &["add", "-A"]);
    assert!(index_gitlinks(tmp.path()).contains(&"wt*".to_string()));

    let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    let lines = printed_lines(&text, "git --literal-pathspecs rm --cached -f -- 'wt*'");
    assert_eq!(lines.len(), 1, "one literal gitlink line; got:\n{text}");
    assert_eq!(
        lines[0],
        format!("git --literal-pathspecs rm --cached -f -- 'wt*'{GITLINK_LINE_SUFFIX}")
    );
    assert!(
        printed_lines(&text, "git rm --cached -f -- 'wt*'").is_empty(),
        "never the bare glob; got:\n{text}"
    );

    let ran = run_printed_line(tmp.path(), &lines[0]);
    assert!(ran.status.success(), "{}", combined(&ran));
    let tracked = hgit(tmp.path(), &["ls-files"]);
    assert!(
        tracked.lines().any(|l| l == "wtA") && tracked.lines().any(|l| l == "wtB/deep/file.js"),
        "the printed line must remove only `wt*`; tracked:\n{tracked}"
    );
    assert!(!index_gitlinks(tmp.path()).contains(&"wt*".to_string()));
    assert!(
        tmp.path().join("wt*/README.md").exists(),
        "the worktree stays"
    );
}

#[test]
fn root_grouped_lines_use_literal_pathspecs_but_rm_rf_does_not() {
    // SEC-O7: Git reads the path literally (`git --literal-pathspecs`, #1536);
    // `rm -rf` takes the plain path.
    let tmp = gitlink_repo();
    write_file(&tmp.path().join("a[1]/dist/plugin.js"), "generated\n");
    hgit(tmp.path(), &["add", "-A"]);

    let output = guard(tmp.path(), &["--mode", "pre-commit"], &[]);
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert_eq!(
        printed_lines(
            &text,
            "git --literal-pathspecs rm -r --cached -- 'a[1]/dist'"
        )
        .len(),
        1,
        "got:\n{text}"
    );
    assert_eq!(
        printed_lines(&text, "rm -rf -- 'a[1]/dist'").len(),
        1,
        "`rm -rf` takes the plain path; got:\n{text}"
    );
    assert!(!text.contains(":(literal)"), "{text}");
    assert!(
        printed_lines(&text, "git rm -r --cached -- 'a[1]/dist'").is_empty(),
        "never the bare glob; got:\n{text}"
    );
}

/// A repository with `files` committed on `main` (pre-existing), and a task
/// branch carrying one unrelated commit. Every file is added with
/// `git add -A`, so no path is read as a pathspec while building it.
fn preexisting_tree_repo(files: &[&str]) -> TempDir {
    let tmp = gitlink_repo();
    write_file(&tmp.path().join("keep/src/lib.rs"), "pub fn keep() {}\n");
    for file in files {
        write_file(&tmp.path().join(file), "vendored\n");
    }
    hgit(tmp.path(), &["add", "-A"]);
    hgit(tmp.path(), &["commit", "-qm", "history: vendored tree"]);
    hgit(tmp.path(), &["checkout", "-q", "-b", "task/dep"]);
    write_file(&tmp.path().join("package.json"), "{}\n");
    hgit(tmp.path(), &["add", "package.json"]);
    hgit(tmp.path(), &["commit", "-qm", "unrelated"]);
    tmp
}

fn cleanup_line(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.split_once("Clean up in a dedicated change: "))
        .map(|(_, command)| command.trim().to_string())
}

#[test]
fn cleanup_line_quotes_pathspecs_and_drops_gitignore_specials() {
    // SEC-O5 / SEC-O7, run as printed: each line untracks exactly its tree,
    // and appends to `.gitignore` (staged with `git add .gitignore`) only for
    // a plain root. Git reads the root literally through
    // `git --literal-pathspecs`, the form #1536 chose (it replaced this PR's
    // `:(literal)` prefix when origin/main was merged in).
    let cases: [(&str, &str, &str); 7] = [
        (
            "node_modules/a/index.js",
            "node_modules",
            "git rm -r --cached -- 'node_modules' && echo 'node_modules/' >> .gitignore \
             && git add .gitignore && git commit -m 'chore: untrack node_modules'",
        ),
        (
            ":!x/node_modules/a/index.js",
            ":!x/node_modules",
            "git --literal-pathspecs rm -r --cached -- ':!x/node_modules' && echo \
             ':!x/node_modules/' >> .gitignore && git add .gitignore && git commit -m \
             'chore: untrack :!x/node_modules'",
        ),
        (
            "vendor/[old]/node_modules/a/index.js",
            "vendor/[old]/node_modules",
            "git --literal-pathspecs rm -r --cached -- 'vendor/[old]/node_modules' && git commit -m \
             'chore: untrack vendor/[old]/node_modules'   # write its .gitignore entry by hand",
        ),
        (
            "a*/node_modules/a/index.js",
            "a*/node_modules",
            "git --literal-pathspecs rm -r --cached -- 'a*/node_modules' && git commit -m \
             'chore: untrack a*/node_modules'   # write its .gitignore entry by hand",
        ),
        (
            "a?/node_modules/a/index.js",
            "a?/node_modules",
            "git --literal-pathspecs rm -r --cached -- 'a?/node_modules' && git commit -m \
             'chore: untrack a?/node_modules'   # write its .gitignore entry by hand",
        ),
        (
            "#x/node_modules/a/index.js",
            "#x/node_modules",
            "git rm -r --cached -- '#x/node_modules' && git commit -m \
             'chore: untrack #x/node_modules'   # write its .gitignore entry by hand",
        ),
        (
            "!x/node_modules/a/index.js",
            "!x/node_modules",
            "git rm -r --cached -- '!x/node_modules' && git commit -m \
             'chore: untrack !x/node_modules'   # write its .gitignore entry by hand",
        ),
    ];

    for (file, root, expected) in cases {
        let tmp = preexisting_tree_repo(&[file]);
        let repo = tmp.path();
        let gitignore_before = fs::read_to_string(repo.join(".gitignore")).expect("read");

        let output = guard(repo, &["--mode", "pre-publish", "--baseline", "main"], &[]);
        let text = combined(&output);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{root}: advisory only\n{text}"
        );
        let cleanup = cleanup_line(&text)
            .unwrap_or_else(|| panic!("{root}: clean-up line missing; got:\n{text}"));
        assert_eq!(cleanup, expected, "{root}: the clean-up line, verbatim");

        let ran = run_printed_line(repo, &cleanup);
        assert!(
            ran.status.success(),
            "{root}: {cleanup}\n{}",
            combined(&ran)
        );
        let tracked = hgit(repo, &["ls-files"]);
        assert!(
            !tracked.contains("/a/index.js"),
            "{root}: the tree is untracked; tracked:\n{tracked}"
        );
        for kept in ["README.md", ".gitignore", "keep/src/lib.rs", "package.json"] {
            assert!(
                tracked.lines().any(|line| line == kept),
                "{root}: `{kept}` must stay tracked; tracked:\n{tracked}"
            );
        }
        assert_eq!(
            hgit(repo, &["log", "-1", "--format=%s"]).trim(),
            format!("chore: untrack {root}")
        );
        let gitignore_after = fs::read_to_string(repo.join(".gitignore")).expect("read");
        let committed = hgit(repo, &["show", "--name-only", "--format=", "HEAD"]);
        if expected.contains(">> .gitignore") {
            assert_eq!(
                gitignore_after,
                format!("{gitignore_before}{root}/\n"),
                "{root}"
            );
            assert!(
                committed.lines().any(|line| line == ".gitignore"),
                "{root}: the .gitignore change is committed with the untracking:\n{committed}"
            );
        } else {
            assert_eq!(
                gitignore_after, gitignore_before,
                "{root}: .gitignore untouched"
            );
        }
    }
}

#[test]
fn unsafe_preexisting_root_gets_no_cleanup_line() {
    // SEC-O3: the rest of the `Way forward` block stays.
    let tmp = preexisting_tree_repo(&["a\u{202e}b/node_modules/a/index.js"]);

    let output = guard(
        tmp.path(),
        &["--mode", "pre-publish", "--baseline", "main"],
        &[],
    );
    let text = combined(&output);
    assert_eq!(output.status.code(), Some(0), "{text}");
    assert!(
        text.contains(r"a\u{202e}b/node_modules") && !text.contains('\u{202e}'),
        "the root is printed escaped; got:\n{text}"
    );
    assert!(
        cleanup_line(&text).is_none(),
        "no clean-up line for an unsafe root; got:\n{text}"
    );
    for kept in [
        "Way forward",
        "List every path and fail closed on the whole condition: amplihack hygiene artifact-guard --repo '",
        "--preexisting block",
    ] {
        assert!(text.contains(kept), "`{kept}` stays; got:\n{text}");
    }
}
