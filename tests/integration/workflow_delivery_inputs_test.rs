//! `base_branch` and `strict_delivery` (docs/reference/workflow-delivery-inputs.md).
//!
//! Covers the shared helper `amplifier-bundle/tools/workflow_delivery_inputs.sh`,
//! the step-15/16 protected-branch guard, step-06d (goal already met) for an
//! issue owned by another repository, and step-04's new-branch base. Step-03 is
//! covered in `issue_1881_local_default_tracking_test.rs` and step-16 in
//! `issue_2306_publish/recovery.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_yaml::Value;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn helper() -> PathBuf {
    workspace_root().join("amplifier-bundle/tools/workflow_delivery_inputs.sh")
}

fn step_command(recipe: &str, step_id: &str) -> String {
    let path = workspace_root().join(format!("amplifier-bundle/recipes/{recipe}.yaml"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let doc: Value = serde_yaml::from_str(&text).expect("recipe parses");
    doc["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s["id"].as_str() == Some(step_id))
        .and_then(|s| s["command"].as_str())
        .unwrap_or_else(|| panic!("{recipe}: no bash step {step_id}"))
        .to_owned()
}

fn bash(script: &str, cwd: &Path, env: &[(&str, &str)]) -> Output {
    let mut cmd = crate::isolated_amplihack::isolated("bash");
    cmd.arg("-c").arg(script).current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn bash")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn describe(out: &Output) -> String {
    format!(
        "status={}\nstdout={}\nstderr={}",
        out.status,
        text(&out.stdout),
        text(&out.stderr)
    )
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command()
        .args(["-c", "user.email=test@test", "-c", "user.name=test"])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}: {}", describe(&out));
    text(&out.stdout).trim().to_owned()
}

/// A checkout whose bare origin has `main` and a `dev` that is one commit
/// ahead of it, with remote-tracking refs for both. No network.
struct Repo {
    _origin: TempDir,
    root: TempDir,
}

impl Repo {
    fn new() -> Self {
        let origin = TempDir::new().unwrap();
        let root = TempDir::new().unwrap();
        let repo = root.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(origin.path(), &["init", "--bare", "-b", "main"]);
        git(&repo, &["init", "-b", "main"]);
        git(
            &repo,
            &["remote", "add", "origin", origin.path().to_str().unwrap()],
        );
        fs::write(repo.join("README.md"), "init\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "init"]);
        git(&repo, &["push", "-u", "origin", "HEAD:main"]);
        git(&repo, &["checkout", "-b", "dev"]);
        fs::write(repo.join("dev-only.txt"), "dev\n").unwrap();
        git(&repo, &["add", "dev-only.txt"]);
        git(&repo, &["commit", "-m", "dev work"]);
        git(&repo, &["push", "origin", "HEAD:dev"]);
        git(&repo, &["checkout", "main"]);
        git(&repo, &["branch", "-D", "dev"]);
        git(&repo, &["remote", "set-head", "origin", "main"]);
        Repo {
            _origin: origin,
            root,
        }
    }

    fn path(&self) -> PathBuf {
        self.root.path().join("repo")
    }

    fn rev(&self, name: &str) -> String {
        git(&self.path(), &["rev-parse", name])
    }
}

// ---------------------------------------------------------------------------
// workflow_delivery_inputs.sh
// ---------------------------------------------------------------------------

fn source_and_run(repo: &Repo, call: &str) -> Output {
    let script = format!("set -euo pipefail\n. \"$HELPER\"\n{call}\n");
    bash(
        &script,
        &repo.path(),
        &[("HELPER", helper().to_str().unwrap())],
    )
}

#[test]
fn delivery_base_ref_prints_the_remote_tracking_base() {
    let repo = Repo::new();
    let out = source_and_run(&repo, "delivery_base_ref dev feat/x 'release/*'");
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(text(&out.stdout), "origin/dev\n");
}

#[test]
fn delivery_base_ref_refuses_without_substituting_another_base() {
    let repo = Repo::new();
    // A file that the forbidden glob `*` would expand to, were the list split
    // by an unquoted expansion: the pattern must still match `dev` itself.
    fs::write(repo.path().join("zz-not-a-branch"), "").unwrap();
    for (call, reason) in [
        (
            "delivery_base_ref 'a..b' '' ''",
            "is not a valid branch name",
        ),
        ("delivery_base_ref '-x' '' ''", "is not a valid branch name"),
        (
            "delivery_base_ref refs/heads/dev '' ''",
            "must be a plain branch name",
        ),
        ("delivery_base_ref @ '' ''", "must be a plain branch name"),
        (
            "delivery_base_ref dev dev ''",
            "is the branch being delivered",
        ),
        (
            "delivery_base_ref dev '' 'release/* dev'",
            "matches forbidden_base_branches pattern 'dev'",
        ),
        (
            "delivery_base_ref dev '' '*'",
            "matches forbidden_base_branches pattern '*'",
        ),
        (
            "delivery_base_ref nope '' ''",
            "has no remote-tracking ref origin/nope",
        ),
    ] {
        let out = source_and_run(&repo, call);
        assert_eq!(out.status.code(), Some(1), "{call}: {}", describe(&out));
        assert!(out.stdout.is_empty(), "{call}: no base is printed");
        assert!(
            text(&out.stderr).contains(reason),
            "{call}: expected {reason:?}\n{}",
            describe(&out)
        );
    }
}

#[test]
fn strict_issue_tag_qualifies_only_an_issue_in_another_repository() {
    let repo = Repo::new();
    for (call, expected) in [
        ("strict_issue_tag 42 owner/repo owner/repo", "#42"),
        ("strict_issue_tag 42 Owner/Repo owner/repo", "#42"),
        ("strict_issue_tag 42 plan/tasks owner/repo", "plan/tasks#42"),
    ] {
        let out = source_and_run(&repo, call);
        assert!(out.status.success(), "{call}: {}", describe(&out));
        assert_eq!(text(&out.stdout).trim(), expected, "{call}");
    }
}

#[test]
fn sourcing_the_helper_defines_functions_only() {
    let repo = Repo::new();
    let out = source_and_run(&repo, "true");
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        out.stdout.is_empty() && out.stderr.is_empty(),
        "{}",
        describe(&out)
    );
}

#[test]
fn worktree_mode_requires_existing_branch_under_strict_delivery() {
    let repo = Repo::new();
    let run = |args: &[&str]| {
        let mut cmd = crate::isolated_amplihack::isolated("bash");
        cmd.arg(helper()).args(args).current_dir(repo.path());
        cmd.output().unwrap()
    };
    let out = run(&["worktree", "", "true", "", ""]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(text(&out.stderr).contains("strict_delivery=true requires existing_branch"));
    let out = run(&["worktree", "", "false", "", ""]);
    assert!(
        out.status.success() && out.stdout.is_empty(),
        "{}",
        describe(&out)
    );
    let out = run(&["worktree", "dev", "true", "feat/x", ""]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(text(&out.stdout), "origin/dev\n");
}

// ---------------------------------------------------------------------------
// step-15 / step-16 protected-branch guard
// ---------------------------------------------------------------------------

fn guard_line(step_id: &str) -> String {
    let command = step_command("workflow-publish", step_id);
    let lines: Vec<&str> = command
        .lines()
        .filter(|l| {
            l.trim_start()
                .starts_with("if [ \"$_CURRENT_BRANCH\" = \"main\" ]")
        })
        .collect();
    assert_eq!(lines.len(), 1, "{step_id}: one protected-branch guard");
    lines[0].trim().to_owned()
}

#[test]
fn protected_branch_guard_refuses_main_master_and_base_branch_together() {
    let dir = TempDir::new().unwrap();
    for step in ["step-15-commit-push", "step-16-create-draft-pr"] {
        let guard = guard_line(step);
        for (base, current, refused) in [
            ("", "main", true),
            ("", "master", true),
            ("", "feat/x", false),
            ("dev", "main", true),
            ("dev", "master", true),
            ("dev", "dev", true),
            ("dev", "feat/x", false),
        ] {
            let script = format!("_CURRENT_BRANCH=\"$CUR\"\n{guard}\necho allowed\n");
            let out = bash(
                &script,
                dir.path(),
                &[("CUR", current), ("RECIPE_VAR_base_branch", base)],
            );
            assert_eq!(
                !out.status.success(),
                refused,
                "{step}: base={base:?} current={current:?}\n{}",
                describe(&out)
            );
            if refused {
                assert!(text(&out.stderr).contains("protected branch"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// step-06d: goal already met
// ---------------------------------------------------------------------------

/// Fake gh for step-06d. Without `--repo`, issue N is this repository's
/// issue N: CLOSED by a merged pull request. With `--repo github.com/plan/tasks`
/// it is the strict run's issue: OPEN. Prints what `--jq` would.
const FAKE_GH_06D: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
case "$*" in
  "issue view 2306 --repo github.com/plan/tasks --json state --jq .state") echo OPEN ;;
  "issue view 2306 --json state --jq .state") echo CLOSED ;;
  "issue view 2306 --json closedByPullRequestsReferences --jq "*) echo "2313 rysweet/amplihack-rs" ;;
  "pr view 2313 --repo rysweet/amplihack-rs --json mergedAt --jq .mergedAt") echo "2026-11-10T00:00:00Z" ;;
  *) echo "fake gh: unexpected: $*" >&2; exit 7 ;;
esac
"#;

fn run_06d(strict: bool) -> (Output, String) {
    let dir = TempDir::new().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(&gh, FAKE_GH_06D).unwrap();
    fs::set_permissions(
        &gh,
        <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();
    let log = dir.path().join("gh.log");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = bash(
        &step_command("workflow-design", "step-06d-goal-already-met-probe"),
        dir.path(),
        &[
            ("PATH", &path),
            ("GH_LOG", log.to_str().unwrap()),
            ("ISSUE_NUMBER", "2306"),
            (
                "RECIPE_VAR_strict_delivery",
                if strict { "true" } else { "false" },
            ),
            ("RECIPE_VAR_repository", "plan/tasks"),
        ],
    );
    (out, fs::read_to_string(log).unwrap_or_default())
}

#[test]
fn goal_probe_reads_a_strict_issue_in_its_own_repository() {
    // rysweet/amplihack-rs#2306 is closed by a merged PR; plan/tasks#2306 is
    // open. A strict run for plan/tasks#2306 must not skip implementation.
    let (out, log) = run_06d(true);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        text(&out.stdout).trim(),
        "false",
        "{}\n{log}",
        describe(&out)
    );
    assert_eq!(
        log.trim(),
        "issue view 2306 --repo github.com/plan/tasks --json state --jq .state",
        "only the owning repository's issue is read"
    );
}

#[test]
fn goal_probe_without_strict_delivery_still_reads_this_repository() {
    // Control: the same fixture, run as an ordinary caller, finds the local
    // closed issue, so the strict case above is not passing by accident.
    let (out, log) = run_06d(false);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        text(&out.stdout).trim(),
        "true",
        "{}\n{log}",
        describe(&out)
    );
    assert!(!log.contains("--repo github.com/plan/tasks"), "{log}");
}

// ---------------------------------------------------------------------------
// step-04: the base of a new branch
// ---------------------------------------------------------------------------

fn run_step04(repo: &Repo, env: &[(&str, &str)]) -> Output {
    let home = repo.root.path().join("home");
    let runtime = repo.root.path().join("runtime");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    let repo_path = repo.path();
    let mut all: Vec<(&str, &str)> = vec![
        ("REPO_PATH", repo_path.to_str().unwrap()),
        ("BRANCH_PREFIX", "feat"),
        ("ISSUE_NUMBER", "77"),
        ("TASK_DESCRIPTION", "deliver to the dev base"),
        ("EXISTING_BRANCH", ""),
        ("PR_NUMBER", ""),
        ("HOME", home.to_str().unwrap()),
        ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
    ];
    let root = workspace_root();
    all.push(("AMPLIHACK_HOME", root.to_str().unwrap()));
    for (k, v) in env {
        all.retain(|(key, _)| key != k);
        all.push((k, v));
    }
    bash(
        &step_command("workflow-worktree", "step-04-setup-worktree"),
        &repo_path,
        &all,
    )
}

fn receipt(out: &Output) -> serde_json::Value {
    let stdout = text(&out.stdout);
    let start = stdout.find('{').expect("receipt JSON");
    serde_json::from_str(stdout[start..].trim()).expect("receipt parses")
}

#[test]
fn new_branch_starts_from_base_branch_and_the_receipt_says_so() {
    let repo = Repo::new();
    let out = run_step04(&repo, &[("RECIPE_VAR_base_branch", "dev")]);
    assert!(out.status.success(), "{}", describe(&out));
    let r = receipt(&out);
    assert_eq!(r["base_ref"], "origin/dev", "{r}");
    assert_eq!(r["base_branch"], "dev", "{r}");
    let wt = PathBuf::from(r["worktree_path"].as_str().unwrap());
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), repo.rev("origin/dev"));
    assert!(
        wt.join("dev-only.txt").exists(),
        "the branch carries dev's work"
    );
}

#[test]
fn new_branch_without_base_branch_keeps_the_remote_default() {
    let repo = Repo::new();
    let out = run_step04(&repo, &[]);
    assert!(out.status.success(), "{}", describe(&out));
    let r = receipt(&out);
    assert_eq!(r["base_ref"], "origin/main", "{r}");
    let wt = PathBuf::from(r["worktree_path"].as_str().unwrap());
    assert_eq!(git(&wt, &["rev-parse", "HEAD"]), repo.rev("origin/main"));
}

#[test]
fn new_branch_never_falls_back_when_base_branch_is_missing_or_forbidden() {
    for env in [
        vec![("RECIPE_VAR_base_branch", "nope")],
        vec![
            ("RECIPE_VAR_base_branch", "dev"),
            ("RECIPE_VAR_forbidden_base_branches", "release/* dev"),
        ],
    ] {
        let repo = Repo::new();
        let out = run_step04(&repo, &env);
        assert_eq!(out.status.code(), Some(1), "{env:?}: {}", describe(&out));
        assert!(out.stdout.is_empty(), "{env:?}: no receipt");
        assert!(
            !repo.path().join("worktrees").exists(),
            "{env:?}: no worktree is created on another base"
        );
    }
}

#[test]
fn strict_delivery_never_derives_a_new_branch() {
    let repo = Repo::new();
    let out = run_step04(
        &repo,
        &[
            ("RECIPE_VAR_strict_delivery", "true"),
            ("RECIPE_VAR_base_branch", "dev"),
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(text(&out.stderr).contains("strict_delivery=true requires existing_branch"));
    assert!(!repo.path().join("worktrees").exists());
}

#[test]
fn existing_branch_receipt_reports_base_branch() {
    let repo = Repo::new();
    git(&repo.path(), &["branch", "feat/assigned", "origin/dev"]);
    git(&repo.path(), &["push", "origin", "feat/assigned"]);
    let out = run_step04(
        &repo,
        &[
            ("EXISTING_BRANCH", "feat/assigned"),
            ("RECIPE_VAR_base_branch", "dev"),
        ],
    );
    assert!(out.status.success(), "{}", describe(&out));
    let r = receipt(&out);
    assert_eq!(r["branch_name"], "feat/assigned", "{r}");
    assert_eq!(r["base_ref"], "origin/dev", "{r}");
    assert_eq!(r["base_branch"], "dev", "{r}");
}
