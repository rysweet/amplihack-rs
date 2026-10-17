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
    for forbidden in ["git rm -r --cached", "git rm --cached", "rm -rf"] {
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
        combined.contains("git rm -r --cached -- 'build'")
            && combined.contains("rm -rf -- 'build'"),
        "the untracked build/ leftover keeps its root remedy; got:\n{combined}"
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
