//! tests/integration/autodrive_pr_worktree_tests.rs
//!
//! Issue #1528: auto-drive phases 2 and 3 run, and their agents commit, only
//! inside the pull request's linked worktree. Never in the launch checkout.
//!
//! - `amplifier-bundle/tools/autodrive_pr_worktree.sh resolve` finds or creates
//!   that worktree and refuses every ambiguous or unsafe case (AC3, SAC1-3,
//!   SEC-1/2/4/9). It writes nothing except `git worktree add` and a fetch into
//!   the remote-tracking ref.
//! - Both loop recipes' step-02 resolve the worktree once, before any round,
//!   and pass it to the rounds as `--repo` and `repo_path` (AC2).
//! - Both round step-04 commit blocks check, before staging anything, that they
//!   stand in a linked worktree on the PR head branch, and run the guard before
//!   and after `git add -A`. Every check stops the shell (AC2, SAC4, SEC-5/6).
//!
//! Every test is hermetic: temporary repositories with a local bare `origin`,
//! a stub `gh` first on PATH, an isolated HOME and git config, no network. In
//! every resolver test the main checkout's HEAD and index are unchanged.

#![cfg(unix)]
#![allow(clippy::too_many_lines)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use regex::Regex;
use serde_yaml::Value;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack -> bins
    path.pop(); // bins -> workspace root
    path
}

fn tools_dir() -> PathBuf {
    workspace_root().join("amplifier-bundle/tools")
}

fn helper() -> PathBuf {
    tools_dir().join("autodrive_pr_worktree.sh")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load_recipe(name: &str) -> Value {
    let path = workspace_root().join("amplifier-bundle/recipes").join(name);
    serde_yaml::from_str(&read(&path)).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn step_field(recipe: &str, id: &str, field: &str) -> String {
    load_recipe(recipe)
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe has steps")
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|step| step.get(field).and_then(Value::as_str))
        .unwrap_or_else(|| panic!("{recipe}: step `{id}` must have a `{field}`"))
        .to_string()
}

fn write_exec(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("mkdir {}: {e}", parent.display()));
    }
    fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|e| panic!("chmod {}: {e}", path.display()));
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn describe(output: &Output) -> String {
    format!(
        "exit {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        text(&output.stdout),
        text(&output.stderr)
    )
}

// ---------------------------------------------------------------------------
// Hermetic sandbox
// ---------------------------------------------------------------------------

/// A stub `gh` that never touches the network. It logs its argv and cwd, and
/// answers `pr view` from files in `$STUB_GH_STATE`: `head` (the head branch),
/// `fail` (exit 1), `cross` (a fork PR) and `crossnull` (a PR whose
/// `isCrossRepository` is missing or `null`). For a fork PR it emulates a
/// filter on the field: a query that applies one prints nothing; a query that
/// does not prints the fork's branch name anyway. For `crossnull` only
/// `select(.isCrossRepository == false)` drops the PR, as jq would: `null` is
/// not `false`, but `null | not` is `true`.
const STUB_GH: &str = r#"#!/usr/bin/env bash
state="${STUB_GH_STATE:?}"
printf '%s\n' "$*" >> "$state/argv.log"
pwd -P >> "$state/cwd.log"
if [ -e "$state/fail" ]; then echo "stub gh: failing as asked" >&2; exit 1; fi
query=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-q" ] || [ "$prev" = "--jq" ]; then query="$arg"; fi
  prev="$arg"
done
if [ -e "$state/cross" ]; then
  case "$query" in
    *isCrossRepository*) exit 0 ;;
  esac
fi
if [ -e "$state/crossnull" ]; then
  case "$query" in
    *"isCrossRepository == false"*) exit 0 ;;
  esac
fi
if [ -e "$state/head" ]; then cat "$state/head"; printf '\n'; fi
exit 0
"#;

struct Sandbox {
    _tmp: TempDir,
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = fs::canonicalize(tmp.path()).expect("canonicalize tempdir");
        for dir in ["home", "bin", "gh-config", "gh-state", "cwd"] {
            fs::create_dir_all(root.join(dir)).expect("create sandbox dir");
        }
        write_exec(&root.join("bin/gh"), STUB_GH);
        Self { _tmp: tmp, root }
    }

    fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    /// Where every command runs unless a test says otherwise. Not a git repo,
    /// so a stray `touch PWNED` or `git add` lands somewhere visible.
    fn cwd(&self) -> PathBuf {
        self.root.join("cwd")
    }

    fn gh_state(&self) -> PathBuf {
        self.root.join("gh-state")
    }

    fn isolate<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        amplihack_git::scrub(command)
            .env("HOME", self.root.join("home"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.bin().display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Auto Drive Test")
            .env("GIT_AUTHOR_EMAIL", "auto-drive@example.invalid")
            .env("GIT_COMMITTER_NAME", "Auto Drive Test")
            .env("GIT_COMMITTER_EMAIL", "auto-drive@example.invalid")
            .env("GH_CONFIG_DIR", self.root.join("gh-config"))
            .env("STUB_GH_STATE", self.gh_state())
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_HOST")
            .env_remove("GH_REPO")
            .env_remove("AMPLIHACK_HOME")
            .env_remove("REPO_PATH")
    }

    fn git_output(&self, dir: &Path, args: &[&str]) -> Output {
        let mut command = amplihack_git::command();
        command.args(args).current_dir(dir);
        self.isolate(&mut command)
            .output()
            .unwrap_or_else(|e| panic!("run git {args:?} in {}: {e}", dir.display()))
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let output = self.git_output(dir, args);
        assert!(
            output.status.success(),
            "git {args:?} in {}: {}",
            dir.display(),
            describe(&output)
        );
        text(&output.stdout)
    }

    fn gh_head(&self, head: &str) {
        fs::write(self.gh_state().join("head"), head).expect("write stub gh head");
    }

    fn gh_fail(&self) {
        fs::write(self.gh_state().join("fail"), "").expect("write stub gh fail");
    }

    fn gh_cross(&self) {
        fs::write(self.gh_state().join("cross"), "").expect("write stub gh cross");
    }

    fn gh_cross_null(&self) {
        fs::write(self.gh_state().join("crossnull"), "").expect("write stub gh crossnull");
    }

    fn gh_calls(&self) -> String {
        fs::read_to_string(self.gh_state().join("argv.log")).unwrap_or_default()
    }

    fn gh_cwds(&self) -> String {
        fs::read_to_string(self.gh_state().join("cwd.log")).unwrap_or_default()
    }

    /// Every file or directory under the sandbox whose name contains `needle`.
    fn find_named(&self, needle: &str) -> Vec<PathBuf> {
        fn walk(dir: &Path, needle: &str, out: &mut Vec<PathBuf>) {
            let Ok(entries) = fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_name().to_string_lossy().contains(needle) {
                    out.push(path.clone());
                }
                let is_real_dir = fs::symlink_metadata(&path)
                    .map(|meta| meta.is_dir())
                    .unwrap_or(false);
                if is_real_dir {
                    walk(&path, needle, out);
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.root, needle, &mut found);
        found
    }
}

// ---------------------------------------------------------------------------
// Repository fixture: a bare local origin and a main checkout on `main`
// ---------------------------------------------------------------------------

struct Repo {
    sb: Sandbox,
    origin: PathBuf,
    main: PathBuf,
}

fn repo() -> Repo {
    repo_named("main")
}

fn repo_named(main_dir: &str) -> Repo {
    let sb = Sandbox::new();
    let origin = sb.root.join("origin.git");
    let origin_str = origin.to_str().expect("utf-8 path").to_string();
    sb.git(
        &sb.root,
        &["init", "-q", "--bare", "-b", "main", &origin_str],
    );
    let main = sb.root.join(main_dir);
    fs::create_dir_all(&main).expect("create main checkout");
    sb.git(&main, &["init", "-q", "-b", "main"]);
    fs::write(main.join("README.md"), "# fixture\n").expect("write README");
    sb.git(&main, &["add", "README.md"]);
    sb.git(&main, &["commit", "-qm", "initial"]);
    sb.git(&main, &["remote", "add", "origin", &origin_str]);
    sb.git(&main, &["push", "-q", "-u", "origin", "main"]);
    Repo { sb, origin, main }
}

impl Repo {
    fn main_str(&self) -> &str {
        self.main.to_str().expect("utf-8 path")
    }

    /// Push `branch`, one commit ahead of `main`, from a separate clone. The
    /// main checkout has neither the local branch nor the remote-tracking ref
    /// afterwards: the branch was pushed after the main checkout last fetched.
    fn push_from_elsewhere(&self, branch: &str) {
        let pusher = self.sb.root.join("pusher");
        if !pusher.exists() {
            self.sb.git(
                &self.sb.root,
                &[
                    "clone",
                    "-q",
                    self.origin.to_str().expect("utf-8 path"),
                    pusher.to_str().expect("utf-8 path"),
                ],
            );
        }
        self.sb
            .git(&pusher, &["checkout", "-q", "--detach", "origin/main"]);
        fs::write(pusher.join("branch-work.txt"), branch).expect("write branch work");
        self.sb.git(&pusher, &["add", "branch-work.txt"]);
        self.sb.git(&pusher, &["commit", "-qm", "branch work"]);
        self.sb.git(
            &pusher,
            &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")],
        );
    }

    fn local_branch(&self, branch: &str) {
        self.sb.git(&self.main, &["branch", branch, "main"]);
    }

    fn add_worktree(&self, path: &Path, branch: &str) {
        self.sb.git(
            &self.main,
            &[
                "worktree",
                "add",
                "-q",
                path.to_str().expect("utf-8 path"),
                branch,
            ],
        );
    }

    fn has_ref(&self, name: &str) -> bool {
        self.sb
            .git_output(&self.main, &["show-ref", "--verify", "--quiet", name])
            .status
            .success()
    }

    /// The main checkout's HEAD and index. Must never change.
    fn snapshot(&self) -> String {
        format!(
            "HEAD {}\nbranch {}\nindex\n{}\ncached\n{}",
            self.sb.git(&self.main, &["rev-parse", "HEAD"]),
            self.sb
                .git(&self.main, &["rev-parse", "--abbrev-ref", "HEAD"]),
            self.sb.git(&self.main, &["ls-files", "-s"]),
            self.sb.git(&self.main, &["diff", "--cached", "--raw"]),
        )
    }

    fn worktree_records(&self) -> usize {
        self.sb
            .git(&self.main, &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count()
    }

    fn resolve(&self, args: &[&str]) -> Output {
        let mut command = Command::new("bash");
        command.arg(helper()).args(args).current_dir(self.sb.cwd());
        self.sb
            .isolate(&mut command)
            .output()
            .expect("run autodrive_pr_worktree.sh")
    }

    fn resolve_head(&self, head: &str) -> Output {
        self.resolve(&["resolve", "--repo", self.main_str(), "--head", head])
    }

    fn resolve_pr(&self, pr: &str) -> Output {
        self.resolve(&["resolve", "--repo", self.main_str(), "--pr", pr])
    }
}

fn canonical(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|e| panic!("canonicalize {}: {e}", path.display()))
        .display()
        .to_string()
}

/// A refusal: the expected exit code and nothing at all on stdout (SEC-4: a
/// caller capturing stdout must never mistake a diagnostic for a path).
fn assert_refused(output: &Output, code: i32, context: &str) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "{context}: expected exit {code}\n{}",
        describe(output)
    );
    assert!(
        output.stdout.is_empty(),
        "{context}: a refusal prints nothing on stdout\n{}",
        describe(output)
    );
}

/// A success: exit 0 and exactly one line on stdout, the absolute path.
fn resolved_path(output: &Output, context: &str) -> String {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{context}\n{}",
        describe(output)
    );
    let stdout = text(&output.stdout);
    assert!(
        stdout.ends_with('\n') && stdout.matches('\n').count() == 1,
        "{context}: stdout must be exactly one line (SEC-4/SAC3)\n{}",
        describe(output)
    );
    let path = stdout.trim_end_matches('\n').to_string();
    assert!(
        path.starts_with('/'),
        "{context}: path must be absolute: {path}"
    );
    path
}

// ---------------------------------------------------------------------------
// AC3: resolver behaviour
// ---------------------------------------------------------------------------

#[test]
fn resolves_an_existing_linked_worktree_on_the_branch() {
    // AC3 (a). The linked worktree may live anywhere; the helper reports it.
    let repo = repo();
    repo.local_branch("feat/x");
    let side = repo.sb.root.join("side/x");
    repo.add_worktree(&side, "feat/x");
    let before = repo.snapshot();
    let records = repo.worktree_records();

    let path = resolved_path(&repo.resolve_head("feat/x"), "--repo <main>");
    assert_eq!(path, canonical(&side));

    // Any checkout of the repository works as --repo.
    let output = repo.resolve(&[
        "resolve",
        "--repo",
        side.to_str().expect("utf-8"),
        "--head",
        "feat/x",
    ]);
    assert_eq!(resolved_path(&output, "--repo <linked>"), canonical(&side));

    assert_eq!(repo.worktree_records(), records, "nothing registered");
    assert_eq!(repo.snapshot(), before, "main checkout untouched");
}

#[test]
fn refuses_when_only_the_main_checkout_is_on_the_branch() {
    // AC3 (b1). The core #1528 rule: never fall back to the launch checkout,
    // and never force a second checkout of the branch. The branch is on origin
    // too, so creating a worktree is not what stops it.
    let repo = repo();
    repo.sb.git(&repo.main, &["checkout", "-q", "-b", "feat/x"]);
    repo.sb
        .git(&repo.main, &["push", "-q", "-u", "origin", "feat/x"]);
    let before = repo.snapshot();
    let records = repo.worktree_records();

    let output = repo.resolve_head("feat/x");

    assert_refused(&output, 1, "main checkout on the branch");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("main checkout") && stderr.contains("#1528"),
        "the refusal must say to move the main checkout off the branch:\n{stderr}"
    );
    assert_eq!(repo.worktree_records(), records, "no worktree registered");
    assert!(!repo.main.join("worktrees").exists());
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refuses_when_the_target_path_is_a_worktree_on_another_branch() {
    // AC3 (b2).
    let repo = repo();
    repo.push_from_elsewhere("feat/x");
    let target = repo.main.join("worktrees/feat/x");
    repo.sb.git(
        &repo.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            target.to_str().expect("utf-8"),
        ],
    );
    let before = repo.snapshot();
    let records = repo.worktree_records();

    assert_refused(
        &repo.resolve_head("feat/x"),
        1,
        "target is another branch's worktree",
    );
    assert_eq!(repo.worktree_records(), records);
    assert_eq!(
        repo.sb.git(&target, &["branch", "--show-current"]).trim(),
        "other",
        "the existing worktree must be left on its branch"
    );
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn creates_a_tracking_worktree_when_the_branch_exists_only_on_origin() {
    // AC3 (d) and SAC3.
    let repo = repo();
    repo.push_from_elsewhere("feat/x");
    assert!(!repo.has_ref("refs/heads/feat/x"));
    assert!(!repo.has_ref("refs/remotes/origin/feat/x"));
    let before = repo.snapshot();
    let records = repo.worktree_records();

    let output = repo.resolve_head("feat/x");
    let path = resolved_path(&output, "branch only on origin");

    let expected = repo.main.join("worktrees/feat/x");
    assert_eq!(path, canonical(&expected));
    let wt = PathBuf::from(&path);
    assert_eq!(
        repo.sb.git(&wt, &["branch", "--show-current"]).trim(),
        "feat/x"
    );
    assert_eq!(
        repo.sb
            .git(
                &wt,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}"
                ]
            )
            .trim(),
        "origin/feat/x",
        "the new branch must track origin/feat/x"
    );
    assert_eq!(
        repo.sb.git(&wt, &["rev-parse", "HEAD"]),
        repo.sb
            .git(&repo.origin, &["rev-parse", "refs/heads/feat/x"]),
        "the worktree must be at origin's head of the branch"
    );
    assert_eq!(repo.worktree_records(), records + 1);
    assert_eq!(repo.snapshot(), before, "main checkout untouched");
}

#[test]
fn attaches_an_existing_local_branch_without_a_worktree() {
    let repo = repo();
    repo.local_branch("feat/y");
    let before = repo.snapshot();

    let path = resolved_path(&repo.resolve_head("feat/y"), "local branch");

    assert_eq!(path, canonical(&repo.main.join("worktrees/feat/y")));
    assert_eq!(
        repo.sb
            .git(Path::new(&path), &["branch", "--show-current"])
            .trim(),
        "feat/y"
    );
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refuses_when_the_target_path_exists_unregistered() {
    // AC3 (e): an existing path is left untouched, whatever it is.
    let repo = repo();
    repo.push_from_elsewhere("feat/x");
    repo.push_from_elsewhere("feat/y");
    let keep = repo.main.join("worktrees/feat/x/keep.txt");
    fs::create_dir_all(keep.parent().expect("parent")).expect("mkdir");
    fs::write(&keep, "operator data\n").expect("write keep");
    let dangling = repo.main.join("worktrees/feat/y");
    std::os::unix::fs::symlink(repo.sb.root.join("nowhere"), &dangling).expect("symlink");
    let before = repo.snapshot();
    let records = repo.worktree_records();

    assert_refused(&repo.resolve_head("feat/x"), 1, "existing directory");
    assert_refused(&repo.resolve_head("feat/y"), 1, "dangling symlink");

    assert_eq!(read(&keep), "operator data\n");
    assert_eq!(
        fs::read_dir(keep.parent().expect("parent"))
            .expect("read dir")
            .count(),
        1,
        "nothing may be added to the existing directory"
    );
    assert!(
        fs::symlink_metadata(&dangling)
            .expect("symlink still there")
            .file_type()
            .is_symlink()
    );
    assert!(!repo.sb.root.join("nowhere").exists());
    assert_eq!(repo.worktree_records(), records);
    assert!(!repo.has_ref("refs/heads/feat/x"));
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn resolves_the_head_branch_through_gh_for_a_pr() {
    // AC3 (f).
    let repo = repo();
    repo.local_branch("feat/x");
    let side = repo.sb.root.join("side/x");
    repo.add_worktree(&side, "feat/x");
    repo.sb.gh_head("feat/x");
    let before = repo.snapshot();

    let path = resolved_path(&repo.resolve_pr("7"), "--pr 7");

    assert_eq!(path, canonical(&side));
    let calls = repo.sb.gh_calls();
    assert!(
        calls.contains("pr view 7")
            && calls.contains("headRefName")
            && calls.contains("isCrossRepository"),
        "gh must be asked for the head branch and whether the PR is a fork:\n{calls}"
    );
    assert!(
        repo.sb
            .gh_cwds()
            .lines()
            .all(|cwd| cwd == canonical(&repo.main)),
        "gh must run inside --repo so it resolves that repository's PR: {}",
        repo.sb.gh_cwds()
    );
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refuses_a_pr_gh_cannot_resolve() {
    type Setup = fn(&Sandbox);
    let cases: [(&str, Setup); 2] = [
        ("gh fails", |sb| sb.gh_fail()),
        ("gh prints nothing", |sb| sb.gh_head("")),
    ];
    for (label, setup) in cases {
        let repo = repo();
        setup(&repo.sb);
        let before = repo.snapshot();
        let records = repo.worktree_records();

        assert_refused(&repo.resolve_pr("7"), 1, label);
        assert_eq!(repo.worktree_records(), records, "{label}");
        assert_eq!(repo.snapshot(), before, "{label}");
    }
}

#[test]
fn refuses_a_cross_repository_pr() {
    // SAC2 / SEC-2: a fork's same-named branch must never attach to origin's.
    let repo = repo();
    repo.push_from_elsewhere("feat/x");
    repo.sb.gh_head("feat/x");
    repo.sb.gh_cross();
    let before = repo.snapshot();
    let records = repo.worktree_records();

    assert_refused(&repo.resolve_pr("7"), 1, "cross-repository PR");

    assert!(
        repo.sb.gh_calls().contains("isCrossRepository"),
        "the gh query must filter out cross-repository PRs:\n{}",
        repo.sb.gh_calls()
    );
    assert_eq!(repo.worktree_records(), records);
    assert!(!repo.has_ref("refs/heads/feat/x"), "nothing created");
    assert!(
        !repo.has_ref("refs/remotes/origin/feat/x"),
        "nothing fetched"
    );
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn usage_errors_exit_2() {
    // AC3 (g).
    let repo = repo();
    let main = repo.main_str().to_string();
    let before = repo.snapshot();
    let records = repo.worktree_records();
    let cases: Vec<Vec<&str>> = vec![
        vec![],
        vec!["resolve"],
        vec!["resolve", "--head", "feat/x"],
        vec!["resolve", "--repo", &main],
        vec!["resolve", "--repo", &main, "--pr", "7", "--head", "feat/x"],
        vec!["resolve", "--repo", &main, "--head", ""],
        vec!["resolve", "--repo", "", "--head", "feat/x"],
        vec!["resolve", "--repo", &main, "--head", "feat/x", "--force"],
        vec!["resolve", "--repo", &main, "--head"],
        vec!["commit", "--repo", &main, "--head", "feat/x"],
        vec!["verify", "--repo", &main, "--head", "feat/x"],
        vec!["resolve", "--repo", &main, "--pr", "7abc"],
        vec!["resolve", "--repo", &main, "--pr", "-1"],
        vec!["resolve", "--repo", &main, "--pr", ""],
        vec!["resolve", "--repo", &main, "--pr", "0"],
        vec!["resolve", "--repo", &main, "--pr", "07"],
        vec!["resolve", "--repo", &main, "--pr", "7 8"],
        vec!["resolve", "--repo", &main, "--pr", "$(id)"],
    ];
    for args in cases {
        assert_refused(&repo.resolve(&args), 2, &format!("usage {args:?}"));
    }
    assert!(
        repo.sb.gh_calls().is_empty(),
        "an invalid --pr must be rejected before gh runs:\n{}",
        repo.sb.gh_calls()
    );
    assert_eq!(repo.worktree_records(), records);
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refuses_a_prunable_match_and_never_prunes() {
    let repo = repo();
    repo.local_branch("feat/x");
    let side = repo.sb.root.join("side/x");
    repo.add_worktree(&side, "feat/x");
    fs::remove_dir_all(&side).expect("remove worktree dir");
    let before = repo.snapshot();
    let records = repo.worktree_records();

    let output = repo.resolve_head("feat/x");

    assert_refused(&output, 1, "prunable match");
    assert!(
        text(&output.stderr).contains("git worktree prune"),
        "the refusal must advise `git worktree prune`\n{}",
        describe(&output)
    );
    assert_eq!(
        repo.worktree_records(),
        records,
        "the helper must never prune the stale registration itself"
    );
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refuses_an_ambiguous_match() {
    let repo = repo();
    repo.local_branch("feat/x");
    let first = repo.sb.root.join("side/a");
    let second = repo.sb.root.join("side/b");
    repo.add_worktree(&first, "feat/x");
    // Only a fixture may force a second checkout of the same branch.
    repo.sb.git(
        &repo.main,
        &[
            "worktree",
            "add",
            "-q",
            "-f",
            second.to_str().expect("utf-8"),
            "feat/x",
        ],
    );
    let before = repo.snapshot();
    let records = repo.worktree_records();

    assert_refused(
        &repo.resolve_head("feat/x"),
        1,
        "two linked worktrees on the branch",
    );
    assert_eq!(repo.worktree_records(), records);
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refuses_branch_names_git_would_reject() {
    let repo = repo();
    let before = repo.snapshot();
    let records = repo.worktree_records();
    for head in [
        "feat..x",
        "feat/x.lock",
        "@{-1}",
        "feat/x/",
        "/feat",
        "feat x",
        "feat~1",
        "feat^",
        "feat:x",
        "feat\\x",
        "feat*",
        "feat?",
        "feat[x",
        "feat/.x",
    ] {
        assert_refused(&repo.resolve_head(head), 1, &format!("--head {head:?}"));
    }
    // A leading dash may be caught as a usage error or a ref error; either way
    // it is refused and nothing happens.
    let output = repo.resolve_head("-x");
    assert!(
        matches!(output.status.code(), Some(1 | 2)) && output.stdout.is_empty(),
        "--head -x must be refused\n{}",
        describe(&output)
    );
    assert_eq!(repo.worktree_records(), records);
    assert!(!repo.main.join("worktrees").exists());
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refuses_shell_metacharacters_in_the_branch_name() {
    // SAC1 / SEC-1. The branch name is attacker-controlled and ends up as text
    // in an agent prompt. Several of these are valid git refs, so they are
    // pushed to origin first: the refusal must come from the allowlist, not
    // from the branch being missing.
    let repo = repo();
    let valid_refs = [
        "feat/$(id)",
        "feat/`id`",
        "feat;id",
        "feat/a'b",
        "feat/a\"b",
        "feat/a$b",
        "feat/a&b",
        "feat/a|b",
    ];
    for name in valid_refs {
        repo.push_from_elsewhere(name);
    }
    let before = repo.snapshot();
    let records = repo.worktree_records();

    let mut heads: Vec<&str> = valid_refs.to_vec();
    heads.extend([
        "feat/$(touch PWNED)",
        "feat/`touch PWNED`",
        "feat;touch PWNED",
        "feat/a\nb",
    ]);
    for head in &heads {
        assert_refused(&repo.resolve_head(head), 1, &format!("--head {head:?}"));
    }
    for head in ["feat/$(touch PWNED)", "feat/a'b", "feat;id"] {
        repo.sb.gh_head(head);
        assert_refused(&repo.resolve_pr("7"), 1, &format!("gh head {head:?}"));
    }

    assert!(
        repo.sb.find_named("PWNED").is_empty(),
        "a branch name must never be executed: {:?}",
        repo.sb.find_named("PWNED")
    );
    assert_eq!(repo.worktree_records(), records, "no worktree registered");
    for name in valid_refs {
        assert!(
            !repo.has_ref(&format!("refs/heads/{name}")),
            "no local branch created for {name:?}"
        );
    }
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refuses_worktree_paths_with_shell_metacharacters() {
    // SEC-1 path safety. The printed path is substituted into a prompt as
    // `cd "{{repo_path}}"`, so a matched worktree path, and a target path,
    // must contain no `"`, `$`, backtick, backslash or control character.
    let repo = repo();
    for (index, dir) in ["a$b", "a\"b", "a`b", "a\\b", "a\tb"].iter().enumerate() {
        let branch = format!("feat/p{index}");
        repo.local_branch(&branch);
        repo.add_worktree(&repo.sb.root.join("side").join(dir), &branch);
        let before = repo.snapshot();
        assert_refused(
            &repo.resolve_head(&branch),
            1,
            &format!("matched worktree at side/{dir:?}"),
        );
        assert_eq!(repo.snapshot(), before);
    }

    // The target path is checked before anything is created.
    let repo = repo_named("ma$in");
    repo.push_from_elsewhere("feat/x");
    let before = repo.snapshot();
    let records = repo.worktree_records();
    assert_refused(&repo.resolve_head("feat/x"), 1, "target under ma$in");
    assert_eq!(repo.worktree_records(), records);
    assert!(!repo.main.join("worktrees/feat/x").exists());
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refuses_a_symlinked_or_non_directory_component_under_worktrees() {
    // SEC-9: `git worktree add` would follow a symlinked intermediate and
    // create the run worktree somewhere else entirely.
    type Setup = fn(&Repo);
    let cases: [(&str, Setup); 3] = [
        ("worktrees/feat is a symlink", |repo: &Repo| {
            let elsewhere = repo.sb.root.join("elsewhere");
            fs::create_dir_all(&elsewhere).expect("mkdir");
            fs::create_dir_all(repo.main.join("worktrees")).expect("mkdir");
            std::os::unix::fs::symlink(&elsewhere, repo.main.join("worktrees/feat"))
                .expect("symlink");
        }),
        ("worktrees is a symlink", |repo: &Repo| {
            let elsewhere = repo.sb.root.join("elsewhere");
            fs::create_dir_all(elsewhere.join("feat")).expect("mkdir");
            std::os::unix::fs::symlink(&elsewhere, repo.main.join("worktrees")).expect("symlink");
        }),
        ("worktrees/feat is a file", |repo: &Repo| {
            fs::create_dir_all(repo.main.join("worktrees")).expect("mkdir");
            fs::write(repo.main.join("worktrees/feat"), "not a dir\n").expect("write");
        }),
    ];
    for (label, setup) in cases {
        let repo = repo();
        repo.push_from_elsewhere("feat/x");
        setup(&repo);
        let before = repo.snapshot();
        let records = repo.worktree_records();

        assert_refused(&repo.resolve_head("feat/x"), 1, label);

        assert_eq!(repo.worktree_records(), records, "{label}");
        assert!(
            !repo.sb.root.join("elsewhere/x").exists()
                && !repo.sb.root.join("elsewhere/feat/x").exists(),
            "{label}: nothing may be created through the symlink"
        );
        assert_eq!(repo.snapshot(), before, "{label}");
    }
}

// ---------------------------------------------------------------------------
// AC3 (c): static contract of the helper script
// ---------------------------------------------------------------------------

/// Comment-free logical lines, with `\` continuations joined.
fn logical_lines(script: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for raw in script.lines() {
        let trimmed = raw.trim();
        if current.is_empty() && (trimmed.is_empty() || trimmed.starts_with('#')) {
            continue;
        }
        if let Some(head) = trimmed.strip_suffix('\\') {
            current.push_str(head);
            current.push(' ');
            continue;
        }
        current.push_str(trimmed);
        lines.push(std::mem::take(&mut current));
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// For each byte of `line`, whether it is literal text (inside single quotes,
/// inside double quotes but outside any `$(...)`, or in a trailing comment)
/// rather than code. Lets the checks below tell `log "run git worktree prune"`
/// from a real `git worktree prune`.
fn literal_mask(line: &str) -> Vec<bool> {
    #[derive(Clone, Copy, PartialEq)]
    enum Ctx {
        Code,
        Double,
        Single,
    }
    let bytes = line.as_bytes();
    let mut mask = vec![false; bytes.len()];
    let mut stack = vec![Ctx::Code];
    let mut i = 0;
    while i < bytes.len() {
        let top = *stack.last().expect("context");
        let byte = bytes[i];
        mask[i] = top != Ctx::Code;
        match top {
            Ctx::Single => {
                if byte == b'\'' {
                    stack.pop();
                }
            }
            Ctx::Double => match byte {
                b'\\' => {
                    i += 1;
                    if i < bytes.len() {
                        mask[i] = true;
                    }
                }
                b'"' => {
                    stack.pop();
                }
                b'$' if bytes.get(i + 1) == Some(&b'(') => {
                    stack.push(Ctx::Code);
                    i += 1;
                }
                _ => {}
            },
            Ctx::Code => match byte {
                b'\\' => i += 1,
                b'\'' => stack.push(Ctx::Single),
                b'"' => stack.push(Ctx::Double),
                b'(' => stack.push(Ctx::Code),
                b')' if stack.len() > 1 => {
                    stack.pop();
                }
                b'#' if i == 0 || bytes[i - 1].is_ascii_whitespace() => {
                    for flag in &mut mask[i..] {
                        *flag = true;
                    }
                    break;
                }
                _ => {}
            },
        }
        i += 1;
    }
    mask
}

/// The argv words of every `git` invocation in code position in `line`.
fn git_invocations(line: &str) -> Vec<Vec<String>> {
    let mask = literal_mask(line);
    let starts = Regex::new(r"(?:^|[\s;&|(`!{])git\s").expect("regex");
    let mut invocations = Vec::new();
    for found in starts.find_iter(line) {
        let at = found.start() + found.as_str().find("git").expect("git in match");
        if mask.get(at).copied().unwrap_or(true) {
            continue;
        }
        let rest = &line[at + 3..];
        let end = rest.find([';', '|', '&', ')', '`']).unwrap_or(rest.len());
        invocations.push(
            rest[..end]
                .split_whitespace()
                .map(|word| word.trim_matches('"').to_string())
                .collect(),
        );
    }
    invocations
}

/// The git subcommand of an invocation and its remaining arguments.
fn subcommand(words: &[String]) -> Option<(&str, &[String])> {
    let mut index = 0;
    while index < words.len() && words[index].starts_with('-') {
        index += if matches!(words[index].as_str(), "-C" | "-c") {
            2
        } else {
            1
        };
    }
    words
        .get(index)
        .map(|sub| (sub.as_str(), &words[index + 1..]))
}

fn is_force_flag(word: &str) -> bool {
    word == "--force"
        || word.starts_with("--force=")
        || word.strip_prefix('-').is_some_and(|short| {
            !short.starts_with('-')
                && !short.is_empty()
                && short.chars().all(|c| c.is_ascii_alphabetic())
                && short.contains('f')
        })
}

#[test]
fn literal_mask_separates_messages_from_commands() {
    // Self-check for the static scanner.
    let message = r#"log "refusing: run git worktree prune (#1528)""#;
    assert!(git_invocations(message).is_empty());
    let command = r#"wt="$(git -C "$repo" worktree list --porcelain)""#;
    let found = git_invocations(command);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(subcommand(&found[0]).map(|(sub, _)| sub), Some("worktree"));
    let chained = r#"[ -d "$x" ] && git -C "$repo" worktree add -f -- "$t" "$h" >&2"#;
    let found = git_invocations(chained);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].iter().any(|word| is_force_flag(word)));
    assert!(git_invocations("# git stash in a comment").is_empty());
}

#[test]
fn helper_never_forces_stages_commits_or_rewrites_checkouts() {
    // AC3 (c) and the M3 rule: the only writes are `git worktree add`, the
    // fetch into the remote-tracking ref and a fast-forward-only fetch into the
    // local branch.
    let script = read(&helper());
    let mut problems = Vec::new();
    let mut adds = 0;
    let mut fetches = 0;

    for line in logical_lines(&script) {
        let mask = literal_mask(&line);
        let code: String = line
            .char_indices()
            .filter(|(at, _)| !mask.get(*at).copied().unwrap_or(true))
            .map(|(_, c)| c)
            .collect();
        for forbidden in [
            "eval ",
            "set -x",
            "set -o xtrace",
            "mapfile",
            "readarray",
            "declare -A",
            "local -A",
            ",,}",
            "^^}",
            "| head",
            "--force",
        ] {
            if code.contains(forbidden) {
                problems.push(format!("`{forbidden}` in: {line}"));
            }
        }

        for words in git_invocations(&line) {
            let Some((sub, args)) = subcommand(&words) else {
                continue;
            };
            if words.iter().any(|word| is_force_flag(word)) {
                problems.push(format!("forced git call: {line}"));
            }
            match sub {
                "add" | "stage" | "commit" | "checkout" | "switch" | "reset" | "stash"
                | "clean" | "rm" | "restore" | "push" | "merge" | "rebase" | "gc" | "prune"
                | "update-index" | "update-ref" | "pull" => {
                    problems.push(format!("forbidden `git {sub}`: {line}"));
                }
                "worktree" => match args.first().map(String::as_str) {
                    Some("list") => {}
                    Some("add") => {
                        adds += 1;
                        if !line.contains(">&2") {
                            problems.push(format!("`worktree add` must write to stderr: {line}"));
                        }
                    }
                    other => problems.push(format!("forbidden `git worktree {other:?}`: {line}")),
                },
                "fetch" => {
                    fetches += 1;
                    if !line.contains(">&2") {
                        problems.push(format!("`fetch` must write to stderr: {line}"));
                    }
                    // A fetch may update a local branch only as a
                    // fast-forward: no `+` on a refspec that writes refs/heads/.
                    if args.iter().any(|arg| {
                        arg.trim_matches('"').starts_with('+') && arg.contains(":refs/heads/")
                    }) {
                        problems.push(format!("forced fetch into a local branch: {line}"));
                    }
                }
                "branch"
                    if args.iter().any(|arg| {
                        matches!(
                            arg.as_str(),
                            "-d" | "-D" | "-m" | "-M" | "--delete" | "--move"
                        )
                    }) =>
                {
                    problems.push(format!("forbidden branch rewrite: {line}"));
                }
                _ => {}
            }
        }
    }

    assert!(
        adds >= 2,
        "expected the two `worktree add` forms, found {adds}"
    );
    assert!(
        fetches >= 1,
        "expected the remote-tracking fetch, found {fetches}"
    );
    assert!(
        problems.is_empty(),
        "autodrive_pr_worktree.sh breaks its contract:\n{}",
        problems.join("\n")
    );
}

#[test]
fn helper_follows_the_house_style_of_workflow_worktree_root() {
    let script = read(&helper());
    assert!(
        script.starts_with("#!/usr/bin/env bash\n"),
        "helper must start with the bundle's bash shebang"
    );
    for marker in ["PROBLEM", "CONTRACT", "READ-ONLY", "#1528"] {
        assert!(
            script.contains(marker),
            "helper header must document `{marker}` like workflow_worktree_root.sh"
        );
    }
    assert!(
        script.contains("workflow_worktree_root.sh"),
        "the worktree root must come from workflow_worktree_root.sh, not a second implementation"
    );
    let mode = |path: &Path| {
        fs::metadata(path)
            .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
            .permissions()
            .mode()
            & 0o111
    };
    assert_ne!(mode(&helper()), 0, "helper must be executable");
    assert_eq!(
        mode(&helper()),
        mode(&tools_dir().join("workflow_worktree_root.sh")),
        "helper file mode must match workflow_worktree_root.sh"
    );
}

// ---------------------------------------------------------------------------
// AC2: loop step-02 resolves the PR worktree and hands it to the rounds
// ---------------------------------------------------------------------------

const LOOPS: [(&str, &str, &str); 2] = [
    (
        "autodrive-crusty-loop.yaml",
        "step-02-crusty-loop",
        "CRUSTY_LOOP_PREFLIGHT",
    ),
    (
        "autodrive-merge-loop.yaml",
        "step-02-merge-ready-loop",
        "MERGE_LOOP_PREFLIGHT",
    ),
];

/// The logical line (continuations joined) that contains byte `at`.
fn logical_line_at(command: &str, at: usize) -> String {
    let start = command[..at].rfind('\n').map_or(0, |nl| nl + 1);
    let mut joined = String::new();
    for line in command[start..].lines() {
        joined.push_str(line.trim());
        joined.push(' ');
        if !line.trim_end().ends_with('\\') {
            break;
        }
    }
    joined
}

/// The `bash "$LOOP_HELPER" ...` invocation, continuation lines joined.
fn loop_invocation(command: &str) -> String {
    let start = command
        .find("bash \"$LOOP_HELPER\"")
        .expect("step-02 invokes the loop helper");
    logical_line_at(command, start)
}

#[test]
fn loop_step_02s_pass_the_resolved_pr_worktree_to_repo_and_repo_path() {
    for (recipe, step, _) in LOOPS {
        let command = step_field(recipe, step, "command");
        let resolve_at = command
            .find("autodrive_pr_worktree.sh")
            .unwrap_or_else(|| panic!("{recipe}: step-02 must resolve the PR worktree"));
        let loop_at = command
            .find("bash \"$LOOP_HELPER\"")
            .expect("loop helper call");
        assert!(
            resolve_at < loop_at,
            "{recipe}: the worktree is resolved before the loop starts"
        );
        let resolve_line = logical_line_at(&command, resolve_at);
        for required in [
            "PR_WORKTREE=\"$(bash \"$(dirname \"$LOOP_HELPER\")/autodrive_pr_worktree.sh\" resolve",
            // The preflight's `repo` (#1543): the PR head's worktree when a
            // PR number was supplied, else repo_path.
            "--repo \"$REPO\"",
            "--pr \"$PR\"",
            "|| {",
            "exit 1",
            "#1528",
        ] {
            assert!(
                resolve_line.contains(required),
                "{recipe}: resolve line must contain `{required}`:\n{resolve_line}"
            );
        }

        let invocation = loop_invocation(&command);
        assert!(
            invocation.contains("--repo \"$PR_WORKTREE\""),
            "{recipe}: rounds run in the PR worktree:\n{invocation}"
        );
        assert!(
            invocation.contains("--context \"repo_path=$PR_WORKTREE\""),
            "{recipe}: round agents get the PR worktree as repo_path:\n{invocation}"
        );
        assert!(
            !invocation.contains("REPO_PATH"),
            "{recipe}: the launch checkout must never reach the rounds:\n{invocation}"
        );
    }
}

/// Run a loop step-02 body under `env -i` with stubs for everything it calls,
/// with `REPO_PATH` set to the launch checkout. Returns the output and the
/// directory holding the stubs' argv logs.
fn run_loop_step_02(
    recipe: &str,
    step: &str,
    preflight_var: &str,
    resolve_fails: bool,
) -> (Output, TempDir) {
    run_loop_step_02_from(recipe, step, preflight_var, resolve_fails, false)
}

/// git, isolated from the user's and the system's configuration and hooks.
fn quiet_git(home: &Path, dir: &Path, args: &[&str]) {
    let output = amplihack_git::command_in(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}\n{}",
        describe(&output)
    );
}

/// As `run_loop_step_02`. The launch checkout is the main checkout of a real
/// repository. With `repo_path_is_worktree`, `REPO_PATH` is a linked worktree
/// of it (`<root>/pr-wt`), which the stub resolver also returns, as when an
/// operator passes `-c repo_path=<PR worktree>` from the main checkout.
/// The real `autodrive_worktree_guard.sh` and `workflow_worktree_root.sh`
/// derive the review dir.
fn run_loop_step_02_from(
    recipe: &str,
    step: &str,
    preflight_var: &str,
    resolve_fails: bool,
    repo_path_is_worktree: bool,
) -> (Output, TempDir) {
    let command = step_field(recipe, step, "command");
    let tmp = TempDir::new().expect("tempdir");
    let root = fs::canonicalize(tmp.path()).expect("canonicalize");
    let tools = root.join("ah/amplifier-bundle/tools");
    let logs = root.join("logs");
    let launch = root.join("launch");
    let home = root.join("home");
    fs::create_dir_all(&logs).expect("mkdir logs");
    fs::create_dir_all(&launch).expect("mkdir launch");
    fs::create_dir_all(&home).expect("mkdir home");
    quiet_git(&home, &launch, &["init", "-q"]);
    quiet_git(
        &home,
        &launch,
        &["commit", "-q", "--allow-empty", "-m", "init"],
    );
    let worktree = root.join("pr-wt");
    if repo_path_is_worktree {
        let wt = worktree.display().to_string();
        quiet_git(
            &home,
            &launch,
            &["worktree", "add", "-q", "-b", "pr-branch", &wt],
        );
    }
    let repo_path = if repo_path_is_worktree {
        &worktree
    } else {
        &launch
    };
    fs::create_dir_all(&tools).expect("mkdir tools");
    // Step-02 takes the review dir from the real guard (#1528).
    for real_tool in ["autodrive_worktree_guard.sh", "workflow_worktree_root.sh"] {
        fs::copy(tools_dir().join(real_tool), tools.join(real_tool))
            .unwrap_or_else(|e| panic!("copy {real_tool}: {e}"));
    }
    // The loop stub also keeps the task it was handed with --task-file
    // (#1570), read while the step's private file still exists.
    write_exec(
        &tools.join("autodrive_loop.sh"),
        "#!/usr/bin/env bash\n\
         printf '%s\\n' \"$@\" > \"$STUB_LOGS/loop.argv\"\n\
         prev=''; for a in \"$@\"; do [ \"$prev\" = --task-file ] && cat -- \"$a\" > \"$STUB_LOGS/loop.task\"; prev=\"$a\"; done\n\
         exit 0\n",
    );
    // Step-02 reads task_description with the real helper (#1570).
    fs::copy(
        tools_dir().join("recipe_context_value.sh"),
        tools.join("recipe_context_value.sh"),
    )
    .expect("copy recipe_context_value.sh");
    // Step-02 passes the launch tree's round recipe by absolute path and
    // refuses when it is missing there (#1543).
    let recipes = root.join("ah/amplifier-bundle/recipes");
    fs::create_dir_all(&recipes).expect("mkdir recipes");
    for round in ["autodrive-crusty-round", "autodrive-merge-round"] {
        fs::write(
            recipes.join(format!("{round}.yaml")),
            format!("name: {round}\n"),
        )
        .expect("write round recipe");
    }
    // The merge loop's before-round step (#1517); it only has to exist here.
    write_exec(
        &tools.join("autodrive_crusty_rereview.sh"),
        "#!/usr/bin/env bash\nexit 0\n",
    );
    write_exec(
        &tools.join("autodrive_pr_worktree.sh"),
        "#!/usr/bin/env bash\n\
         printf '%s\\n' \"$@\" > \"$STUB_LOGS/resolver.argv\"\n\
         if [ \"${STUB_RESOLVE_FAIL:-0}\" = 1 ]; then echo 'stub resolver: refused' >&2; exit 1; fi\n\
         printf '%s\\n' \"${STUB_WT:-/stub/wt}\"\n",
    );
    write_exec(
        &root.join("bin/amplihack"),
        "#!/usr/bin/env bash\n\
         case \"$*\" in\n\
           'orch helper extract-json'*) cat ;;\n\
           *'extract-field --field state_dir'*) printf '%s' /stub/state ;;\n\
           *'extract-field --field pr'*) printf '%s' 7 ;;\n\
           *'extract-field --field repo'*) printf '%s' \"$STUB_PRE_REPO\" ;;\n\
           *) printf 'stub amplihack: unexpected: %s\\n' \"$*\" >&2; exit 97 ;;\n\
         esac\n",
    );

    let output = Command::new("bash")
        .arg("-c")
        .arg(&command)
        .current_dir(&launch)
        .env_clear()
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .env("HOME", &home)
        .env("AMPLIHACK_HOME", root.join("ah"))
        .env("REPO_PATH", repo_path)
        // The preflight's `repo` (#1543) is repo_path: these runs have no
        // operator PR number, so #1543's own resolution did not run.
        .env("STUB_PRE_REPO", repo_path)
        .env(
            preflight_var,
            r#"{"state_dir":"/stub/state","pr":"7","repo":"<stub>"}"#,
        )
        .env("RECIPE_VAR_task_description", "drive PR 7 to merge")
        .env("STUB_LOGS", &logs)
        .env("STUB_RESOLVE_FAIL", if resolve_fails { "1" } else { "0" })
        .envs(repo_path_is_worktree.then_some(("STUB_WT", &worktree)))
        .output()
        .expect("run step-02");
    (output, tmp)
}

#[test]
fn loop_step_02_refuses_before_the_loop_when_resolution_fails() {
    for (recipe, step, preflight_var) in LOOPS {
        let (output, tmp) = run_loop_step_02(recipe, step, preflight_var, true);
        let logs = fs::canonicalize(tmp.path()).expect("canon").join("logs");
        assert_ne!(
            output.status.code(),
            Some(0),
            "{recipe}: a failed resolution must fail the step\n{}",
            describe(&output)
        );
        assert!(
            text(&output.stderr).contains("#1528"),
            "{recipe}: say why\n{}",
            describe(&output)
        );
        assert!(
            logs.join("resolver.argv").exists(),
            "{recipe}: the resolver must have been asked\n{}",
            describe(&output)
        );
        assert!(
            !logs.join("loop.argv").exists(),
            "{recipe}: no round may run in the launch checkout when resolution fails"
        );

        let (output, tmp) = run_loop_step_02(recipe, step, preflight_var, false);
        let root = fs::canonicalize(tmp.path()).expect("canon");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{recipe}\n{}",
            describe(&output)
        );
        let resolver: Vec<String> = read(&root.join("logs/resolver.argv"))
            .lines()
            .map(str::to_string)
            .collect();
        let launch = root.join("launch").display().to_string();
        assert_eq!(
            resolver,
            vec!["resolve", "--repo", &launch, "--pr", "7"],
            "{recipe}: resolver argv"
        );
        let argv: Vec<String> = read(&root.join("logs/loop.argv"))
            .lines()
            .map(str::to_string)
            .collect();
        let repo_at = argv
            .iter()
            .position(|arg| arg == "--repo")
            .expect("loop gets --repo");
        assert_eq!(argv[repo_at + 1], "/stub/wt", "{recipe}: {argv:?}");
        assert!(
            argv.iter().any(|arg| arg == "repo_path=/stub/wt"),
            "{recipe}: {argv:?}"
        );
        // The rounds run in the PR worktree and still get the task, through
        // the task file rather than as an argv element (#1528 with #1570).
        assert!(
            argv.iter().any(|arg| arg == "--task-file"),
            "{recipe}: loop gets --task-file: {argv:?}"
        );
        assert_eq!(
            read(&root.join("logs/loop.task")),
            "drive PR 7 to merge",
            "{recipe}: the task reaches the loop"
        );
        assert!(
            !argv.iter().any(|arg| arg.contains("task_description")),
            "{recipe}: the task never travels as an argv element: {argv:?}"
        );
        // The launch checkout reaches the loop exactly once, as the review
        // dir where the read-only judging agents start (#1528). It
        // never reaches `--repo` or `repo_path`, where commits happen.
        let review_at = argv
            .iter()
            .position(|arg| arg == "--review-dir")
            .unwrap_or_else(|| panic!("{recipe}: loop gets --review-dir: {argv:?}"));
        assert_eq!(argv[review_at + 1], launch, "{recipe}: {argv:?}");
        assert!(
            !argv
                .iter()
                .enumerate()
                .any(|(i, arg)| i != review_at + 1
                    && (arg.contains(&launch) || arg == "repo_path=.")),
            "{recipe}: the launch checkout must reach the loop only as --review-dir: {argv:?}"
        );
    }
}

/// Crusty round 1 on PR #1586: with `repo_path` set to the PR's linked
/// worktree, step-02 used to take the review dir from `pwd` after entering
/// `repo_path`, so the review dir WAS the PR worktree and the loop preflight
/// refused UNTRUSTED_REVIEW_DIR before round 1. The review dir is now the
/// main checkout, and the real preflight accepts it.
#[test]
fn loop_step_02_with_repo_path_set_to_the_pr_worktree_reviews_from_the_main_checkout() {
    for (recipe, step, preflight_var) in LOOPS {
        let (output, tmp) = run_loop_step_02_from(recipe, step, preflight_var, false, true);
        let root = fs::canonicalize(tmp.path()).expect("canon");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{recipe}\n{}",
            describe(&output)
        );
        let main = root.join("launch").display().to_string();
        let worktree = root.join("pr-wt").display().to_string();
        let argv: Vec<String> = read(&root.join("logs/loop.argv"))
            .lines()
            .map(str::to_string)
            .collect();
        let after = |flag: &str| {
            let at = argv
                .iter()
                .position(|arg| arg == flag)
                .unwrap_or_else(|| panic!("{recipe}: loop gets {flag}: {argv:?}"));
            argv[at + 1].clone()
        };
        assert_eq!(after("--review-dir"), main, "{recipe}: {argv:?}");
        assert_eq!(after("--repo"), worktree, "{recipe}: {argv:?}");
        let resolver = read(&root.join("logs/resolver.argv"));
        assert!(
            resolver.lines().any(|line| line == worktree),
            "{recipe}: the resolver is asked about repo_path: {resolver}"
        );

        // The loop's own check, with exactly what step-02 handed the loop.
        let preflight = Command::new("bash")
            .arg(tools_dir().join("autodrive_worktree_guard.sh"))
            .args([
                "loop-preflight",
                "crusty",
                &worktree,
                &main,
                "autodrive-crusty-round",
            ])
            .output()
            .expect("run loop-preflight");
        assert_eq!(
            preflight.status.code(),
            Some(0),
            "{recipe}: the preflight accepts the main checkout as review dir\n{}",
            describe(&preflight)
        );
        assert_eq!(
            text(&preflight.stdout).lines().next(),
            Some(main.as_str()),
            "{recipe}: {}",
            describe(&preflight)
        );

        // The step-02 working directory is no longer the review dir: the
        // same worktree handed in as the review dir is still refused, and
        // the refusal names --review-dir and the preflight repo it is derived
        // from (not repo_path, which may differ when a PR number is given),
        // and not the launch dir.
        let refused = Command::new("bash")
            .arg(tools_dir().join("autodrive_worktree_guard.sh"))
            .args([
                "loop-preflight",
                "crusty",
                &worktree,
                &worktree,
                "autodrive-crusty-round",
            ])
            .output()
            .expect("run loop-preflight");
        assert_eq!(refused.status.code(), Some(1), "{}", describe(&refused));
        let said = text(&refused.stderr);
        assert!(
            said.contains("--review-dir")
                && said.contains("preflight repo")
                && !said.contains("launch auto-drive from"),
            "{recipe}: the refusal points at the setting that matters: {said}"
        );
    }
}

#[test]
fn guard_review_dir_prints_the_main_checkout_or_stops() {
    let tmp = TempDir::new().expect("tempdir");
    let root = fs::canonicalize(tmp.path()).expect("canon");
    let (home, main, wt) = (root.join("home"), root.join("main"), root.join("wt"));
    fs::create_dir_all(&home).expect("mkdir home");
    fs::create_dir_all(&main).expect("mkdir main");
    quiet_git(&home, &main, &["init", "-q"]);
    quiet_git(
        &home,
        &main,
        &["commit", "-q", "--allow-empty", "-m", "init"],
    );
    let wt_str = wt.display().to_string();
    quiet_git(
        &home,
        &main,
        &["worktree", "add", "-q", "-b", "pr", &wt_str],
    );
    let guard = tools_dir().join("autodrive_worktree_guard.sh");
    for repo_path in [&main, &wt, &wt.join(".")] {
        let output = Command::new("bash")
            .arg(&guard)
            .arg("review-dir")
            .arg(repo_path)
            .output()
            .expect("run review-dir");
        assert_eq!(output.status.code(), Some(0), "{}", describe(&output));
        assert_eq!(
            text(&output.stdout).trim_end(),
            main.display().to_string(),
            "repo_path {}",
            repo_path.display()
        );
    }
    let not_git = root.join("not-git");
    fs::create_dir_all(&not_git).expect("mkdir");
    let output = Command::new("bash")
        .arg(&guard)
        .arg("review-dir")
        .arg(&not_git)
        .env("GIT_CEILING_DIRECTORIES", &root)
        .output()
        .expect("run review-dir");
    assert_eq!(output.status.code(), Some(1), "{}", describe(&output));
    assert!(output.stdout.is_empty(), "{}", describe(&output));
    assert!(
        text(&output.stderr).contains("#1528"),
        "{}",
        describe(&output)
    );
}

/// A main checkout with one commit and a linked worktree of it, under `root`.
fn main_and_worktree(home: &Path, main: &Path, wt: &Path) {
    fs::create_dir_all(main).expect("mkdir main");
    quiet_git(home, main, &["init", "-q"]);
    quiet_git(home, main, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let wt_str = wt.display().to_string();
    quiet_git(home, main, &["worktree", "add", "-q", "--detach", &wt_str]);
}

/// Crusty round 3 on PR #1586, item 1: `review-dir` must not take its answer
/// from an inherited `GIT_DIR` (a git hook runs with one set, githooks(5)),
/// and the loop preflight must refuse a review dir in another repository,
/// whose `gh pr diff N` would read that repository's pull request N.
#[test]
fn review_dir_ignores_an_inherited_git_dir_and_the_preflight_refuses_another_repository() {
    let tmp = TempDir::new().expect("tempdir");
    let root = fs::canonicalize(tmp.path()).expect("canon");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("mkdir home");
    let (main, wt) = (root.join("main"), root.join("wt"));
    let (other, other_wt) = (root.join("other"), root.join("other-wt"));
    main_and_worktree(&home, &main, &wt);
    main_and_worktree(&home, &other, &other_wt);
    let guard = tools_dir().join("autodrive_worktree_guard.sh");

    let output = Command::new("bash")
        .arg(&guard)
        .arg("review-dir")
        .arg(&wt)
        .env("GIT_DIR", other.join(".git"))
        .env("GIT_COMMON_DIR", other.join(".git"))
        .env("GIT_WORK_TREE", &other)
        .env("GIT_INDEX_FILE", other.join(".git/index"))
        .output()
        .expect("run review-dir");
    assert_eq!(output.status.code(), Some(0), "{}", describe(&output));
    assert_eq!(
        text(&output.stdout).trim_end(),
        main.display().to_string(),
        "an inherited GIT_DIR must not choose the review dir\n{}",
        describe(&output)
    );

    for (review, why) in [
        (&other, "another repository"),
        (&other_wt, "another repository"),
        (&root, "not the top of a git checkout"),
    ] {
        let refused = Command::new("bash")
            .arg(&guard)
            .arg("loop-preflight")
            .arg("crusty")
            .arg(&wt)
            .arg(review)
            .arg("autodrive-crusty-round")
            .output()
            .expect("run loop-preflight");
        assert_eq!(refused.status.code(), Some(1), "{}", describe(&refused));
        assert!(
            text(&refused.stdout).contains("\"loop_result\":\"UNTRUSTED_REVIEW_DIR\"")
                && text(&refused.stderr).contains(why),
            "review dir {}: {}",
            review.display(),
            describe(&refused)
        );
    }

    // The round's own guard refuses it too, before the judging agent starts.
    let refused = Command::new("bash")
        .arg(&guard)
        .arg("round-guard")
        .env("REVIEW_DIR", &other)
        .env("REPO_PATH", &wt)
        .output()
        .expect("run round-guard");
    assert_eq!(refused.status.code(), Some(1), "{}", describe(&refused));
    assert!(refused.stdout.is_empty(), "{}", describe(&refused));
}

/// Crusty round 3 on PR #1586, item 2: for a bare repository the resolver
/// answers the directory beside the bare git dir, which is no checkout.
/// `review-dir` stops by name rather than start agents there.
#[test]
fn review_dir_stops_on_a_bare_repository_with_a_linked_worktree() {
    let tmp = TempDir::new().expect("tempdir");
    let root = fs::canonicalize(tmp.path()).expect("canon");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("mkdir home");
    let (seed, seed_wt) = (root.join("seed"), root.join("seed-wt"));
    main_and_worktree(&home, &seed, &seed_wt);
    let parent = root.join("src");
    fs::create_dir_all(&parent).expect("mkdir src");
    let bare = parent.join("repo.git");
    let bare_str = bare.display().to_string();
    quiet_git(&home, &seed, &["clone", "-q", "--bare", ".", &bare_str]);
    let wt = bare.join("wt");
    let wt_str = wt.display().to_string();
    quiet_git(
        &home,
        &bare,
        &["worktree", "add", "-q", "--detach", &wt_str],
    );

    let guard = tools_dir().join("autodrive_worktree_guard.sh");
    let output = Command::new("bash")
        .arg(&guard)
        .arg("review-dir")
        .arg(&wt)
        .env("GIT_CEILING_DIRECTORIES", &root)
        .output()
        .expect("run review-dir");
    assert_eq!(output.status.code(), Some(1), "{}", describe(&output));
    assert!(output.stdout.is_empty(), "{}", describe(&output));
    let said = text(&output.stderr);
    assert!(
        said.contains("STOP:") && said.contains("bare repository") && said.contains("#1528"),
        "{}",
        describe(&output)
    );

    // Handed to the loop anyway, the directory beside the bare git dir is refused.
    let refused = Command::new("bash")
        .arg(&guard)
        .arg("loop-preflight")
        .arg("crusty")
        .arg(&wt)
        .arg(&parent)
        .arg("autodrive-crusty-round")
        .env("GIT_CEILING_DIRECTORIES", &root)
        .output()
        .expect("run loop-preflight");
    assert_eq!(refused.status.code(), Some(1), "{}", describe(&refused));
    assert!(
        text(&refused.stdout).contains("UNTRUSTED_REVIEW_DIR"),
        "{}",
        describe(&refused)
    );
}

// ---------------------------------------------------------------------------
// Round 3: judging agents never start in the tree they judge (#1528)
// ---------------------------------------------------------------------------

fn agent_steps(recipe: &str) -> Vec<(String, String)> {
    load_recipe(recipe)
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe has steps")
        .iter()
        .filter(|step| step.get("agent").is_some())
        .map(|step| {
            let id = step
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            let wd = step
                .get("working_dir")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{recipe}: agent step `{id}` must set working_dir"))
                .to_string();
            (id, wd)
        })
        .collect()
}

/// The agent CLI loads AGENTS.md, CLAUDE.md and .claude hooks from its
/// working directory. Only the commit step (step-04) may start in the PR
/// worktree; the review, merge-ready assessment and loop-health analyzer start
/// in the launch-side review dir, so the branch under review never instructs
/// the agent that decides whether it passes or when the loop stops.
#[test]
fn judging_agents_do_not_take_working_dir_from_the_pr_worktree() {
    for (recipe, commit_step) in ROUNDS {
        let steps = agent_steps(recipe);
        assert!(
            steps.len() >= 2,
            "{recipe}: expected a judging and a commit agent: {steps:?}"
        );
        for (id, wd) in &steps {
            if id == commit_step {
                assert_eq!(
                    wd, "{{repo_path}}",
                    "{recipe}: {id} commits in the PR worktree"
                );
            } else {
                assert_eq!(
                    wd, "{{review_dir}}",
                    "{recipe}: judging agent `{id}` must start in review_dir, not the PR worktree"
                );
            }
        }
        // A review_dir default of "." would resolve against the round's
        // working dir, the PR worktree. It must be empty, and refused.
        let ctx = load_recipe(recipe);
        let default = ctx
            .get("context")
            .and_then(|c| c.get("review_dir"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{recipe}: context must declare review_dir"));
        assert_eq!(default, "", "{recipe}: review_dir has no default");
        // The guard runs before any agent and before any sub-recipe. The
        // merge round finds its tools first (step-00-tools-dir, #1517); that
        // step starts no agent.
        let all = ctx
            .get("steps")
            .and_then(Value::as_sequence)
            .unwrap_or_else(|| panic!("{recipe}: no steps"));
        let guard = all
            .iter()
            .position(|s| s.get("id").and_then(Value::as_str) == Some("step-00-review-dir-guard"))
            .unwrap_or_else(|| panic!("{recipe}: no step-00-review-dir-guard"));
        let first_judged = all
            .iter()
            .position(|s| {
                s.get("agent").is_some() || s.get("type").and_then(Value::as_str) == Some("recipe")
            })
            .unwrap_or_else(|| panic!("{recipe}: no agent or recipe step"));
        assert!(
            guard < first_judged,
            "{recipe}: the guard runs before any agent or sub-recipe step"
        );
    }

    let steps = agent_steps("loop-health-evaluator.yaml");
    assert_eq!(
        steps,
        vec![(
            "step-02-evaluate-loop-health".to_string(),
            "{{loop_agent_working_dir}}".to_string()
        )],
        "the analyzer starts in loop_agent_working_dir, never in loop_repo_path"
    );
}

fn run_review_dir_guard(recipe: &str, repo: &Path, review: Option<&Path>) -> Output {
    let command = step_field(recipe, "step-00-review-dir-guard", "command");
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(&command)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("REPO_PATH", repo)
        .env("AUTODRIVE_LAUNCH_TOOLS", launch_tools());
    if let Some(review) = review {
        cmd.env("REVIEW_DIR", review);
    }
    cmd.output().expect("run step-00")
}

/// The launch-side tools dir autodrive_loop.sh hands each round
/// (`autodrive_launch_tools`): this checkout's amplifier-bundle/tools.
fn launch_tools() -> PathBuf {
    workspace_root().join("amplifier-bundle/tools")
}

#[test]
fn round_review_dir_guard_refuses_the_tree_under_review() {
    let tmp = TempDir::new().expect("tempdir");
    let root = fs::canonicalize(tmp.path()).expect("canonicalize");
    let pr = root.join("pr-worktree");
    let launch = root.join("launch");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("mkdir home");
    // The PR worktree is a linked worktree of the launch checkout, as in a
    // real run; the guard accepts only that main checkout as the review dir.
    main_and_worktree(&home, &launch, &pr);
    fs::create_dir_all(pr.join("sub")).expect("mkdir pr/sub");
    // A symlink that points into the PR worktree is still the PR worktree.
    std::os::unix::fs::symlink(&pr, root.join("alias")).expect("symlink");
    // Outside the PR worktree, yet not its main checkout (crusty round 3 on
    // PR #1586): a plain directory, another repository, a sibling worktree.
    fs::create_dir_all(root.join("plain")).expect("mkdir plain");
    main_and_worktree(&home, &root.join("other"), &root.join("other-wt"));
    let sibling = root.join("sibling-wt").display().to_string();
    quiet_git(
        &home,
        &launch,
        &["worktree", "add", "-q", "--detach", &sibling],
    );

    for (recipe, _) in ROUNDS {
        let ok = run_review_dir_guard(recipe, &pr, Some(&launch));
        assert_eq!(
            ok.status.code(),
            Some(0),
            "{recipe}: launch side passes\n{}",
            describe(&ok)
        );
        let cases: [(&str, Option<PathBuf>); 9] = [
            ("missing", None),
            ("empty", Some(PathBuf::new())),
            ("same", Some(pr.clone())),
            ("inside", Some(pr.join("sub"))),
            ("symlinked", Some(root.join("alias"))),
            ("absent", Some(root.join("nope"))),
            ("plain", Some(root.join("plain"))),
            ("other-repo", Some(root.join("other"))),
            ("sibling-worktree", Some(root.join("sibling-wt"))),
        ];
        for (label, review) in &cases {
            let out = run_review_dir_guard(recipe, &pr, review.as_deref());
            assert_ne!(
                out.status.code(),
                Some(0),
                "{recipe}/{label}: must refuse\n{}",
                describe(&out)
            );
            assert!(
                text(&out.stderr).contains("#1528"),
                "{recipe}/{label}: say why\n{}",
                describe(&out)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// AC2 / SAC4: round step-04 commit blocks
// ---------------------------------------------------------------------------

const ROUNDS: [(&str, &str); 2] = [
    ("autodrive-crusty-round.yaml", "step-04-address-concerns"),
    ("autodrive-merge-round.yaml", "step-04-address-blockers"),
];

/// The fenced ```bash block in the step-04 prompt that stages and commits.
fn commit_block(prompt: &str) -> String {
    let mut rest = prompt;
    while let Some(open) = rest.find("```bash\n") {
        let body = &rest[open + "```bash\n".len()..];
        let close = body.find("```").expect("closing fence");
        if body[..close].contains("git add -A") {
            return body[..close].to_string();
        }
        rest = &body[close + 3..];
    }
    panic!("no ```bash block with `git add -A` in prompt:\n{prompt}");
}

/// The prompt with every fenced block removed.
fn prose(prompt: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in prompt.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !fenced {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn position(lines: &[String], context: &str, predicate: impl Fn(&str) -> bool) -> usize {
    lines
        .iter()
        .position(|line| predicate(line))
        .unwrap_or_else(|| panic!("{context}: missing line in block:\n{}", lines.join("\n")))
}

#[test]
fn round_step_04_blocks_check_linked_worktree_and_head_branch_before_staging() {
    for (recipe, step) in ROUNDS {
        let prompt = step_field(recipe, step, "prompt");
        let lines = logical_lines(&commit_block(&prompt));
        let ctx = |what: &str| format!("{recipe} {step}: {what}");

        let cd = position(&lines, &ctx("cd"), |l| {
            l.starts_with("cd \"{{repo_path}}\"")
        });
        assert_eq!(
            cd,
            0,
            "{}",
            ctx("the block must start by entering repo_path")
        );
        // The linked-worktree and head-branch checks live in the launch-side
        // autodrive_worktree_guard.sh; the block calls it, and stops on it.
        let checks = position(&lines, &ctx("commit-dir check"), |l| {
            l.starts_with("bash \"{{autodrive_launch_tools}}/autodrive_worktree_guard.sh\" commit-dir {{pr_number}}")
        });
        let identity = position(&lines, &ctx("identity helper"), |l| {
            l.contains("amplihack_prepare_git_commit_identity")
        });
        let guards: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains("amplihack hygiene artifact-guard"))
            .map(|(index, _)| index)
            .collect();
        let add = position(&lines, &ctx("git add -A"), |l| l == "git add -A");
        let commit = position(&lines, &ctx("git commit"), |l| l.starts_with("git commit"));

        let tool = logical_lines(&read(&launch_tools().join("autodrive_worktree_guard.sh")));
        let linked_line = tool
            .iter()
            .find(|l| l.contains("gd=\"$(git rev-parse --absolute-git-dir)\""))
            .unwrap_or_else(|| panic!("autodrive_worktree_guard.sh: no linked-worktree check"));
        for required in [
            "--path-format=absolute --git-common-dir",
            "case \"$cd\" in /*)",
            "[ \"$gd\" != \"$cd\" ]",
            "*) false",
            "|| stop ",
        ] {
            assert!(
                linked_line.contains(required),
                "{}",
                ctx(&format!("linked check needs `{required}`: {linked_line}"))
            );
        }
        let head_line = tool
            .iter()
            .find(|l| l.contains("gh pr view \"$pr\" --json headRefName"))
            .unwrap_or_else(|| panic!("autodrive_worktree_guard.sh: no head-branch check"));
        for required in [
            "[ -n \"$pr_head\" ]",
            "[ \"$(git branch --show-current)\" = \"$pr_head\" ]",
            "|| stop ",
        ] {
            assert!(
                head_line.contains(required),
                "{}",
                ctx(&format!("head check needs `{required}`: {head_line}"))
            );
        }
        for line in [linked_line, head_line] {
            assert!(
                !line.contains("2>/dev/null") && !line.contains("|| true"),
                "{}",
                ctx(&format!("checks must not be silenced: {line}"))
            );
        }
        for at in [cd, checks] {
            let line = &lines[at];
            assert!(
                line.contains("|| {") && line.contains("STOP") && line.contains("exit 1; }"),
                "{}",
                ctx(&format!("each check must stop the shell: {line}"))
            );
            assert!(
                !line.contains("2>/dev/null") && !line.contains("|| true"),
                "{}",
                ctx(&format!("checks must not be silenced: {line}"))
            );
        }

        assert_eq!(
            guards.len(),
            2,
            "{}",
            ctx("guard runs before and after staging")
        );
        for at in &guards {
            let line = &lines[*at];
            assert!(
                line.starts_with(
                    "amplihack hygiene artifact-guard --repo . --mode pre-commit || {"
                ) && line.contains("STOP")
                    && line.contains("exit 1; }"),
                "{}",
                ctx(&format!("a guard refusal must stop the shell: {line}"))
            );
        }
        assert!(
            cd < checks
                && checks < identity
                && identity < guards[0]
                && guards[0] < add
                && add < guards[1]
                && guards[1] < commit,
            "{}",
            ctx(&format!(
                "order must be cd -> commit-dir check -> identity -> guard -> add -> guard -> commit:\n{}",
                lines.join("\n")
            ))
        );
        assert!(
            prompt.to_ascii_lowercase().contains("one shell command"),
            "{}",
            ctx("the prompt must say the block runs as one shell command")
        );
    }
}

#[test]
fn round_step_04_prompts_forbid_ignored_nested_and_forced_staging() {
    // R4. The wording must not itself trip the forbidden-flag scanners.
    for (recipe, step) in ROUNDS {
        let prompt = step_field(recipe, step, "prompt");
        let prose = prose(&prompt);
        let lower = prose.to_ascii_lowercase();
        for required in [
            "after staging",
            "ignored",
            "worktrees/",
            "another checkout",
            "never force",
            "#1528",
        ] {
            assert!(
                lower.contains(required),
                "{recipe} {step}: the staging rules must mention `{required}`:\n{prose}"
            );
        }
        for forbidden in ["git add", "--force", " -f "] {
            assert!(
                !prose.contains(forbidden),
                "{recipe} {step}: the prose must not contain `{forbidden}`:\n{prose}"
            );
        }
        assert!(
            !prompt.contains("--force") && !prompt.contains("git add -f"),
            "{recipe} {step}: no forced staging anywhere in the prompt"
        );
    }
}

/// A linked worktree at `<main>/worktrees/feat/x` on `feat/x`, tracking
/// `origin/feat/x`, with the stub `gh` answering `feat/x` for PR 7.
fn round_fixture() -> (Repo, PathBuf) {
    let repo = repo();
    repo.local_branch("feat/x");
    repo.sb
        .git(&repo.main, &["push", "-q", "-u", "origin", "feat/x"]);
    let wt = repo.main.join("worktrees/feat/x");
    repo.add_worktree(&wt, "feat/x");
    repo.sb.gh_head("feat/x");
    (repo, wt)
}

/// Run a round's commit block exactly as an agent would: placeholders
/// substituted, one `bash -c`, the cwd somewhere unrelated. The stub
/// `amplihack` logs each guard call and refuses call number `guard_fail_on`.
fn run_round_block(
    recipe: &str,
    step: &str,
    repo: &Repo,
    repo_path: &Path,
    guard_fail_on: u32,
) -> Output {
    run_round_block_env(recipe, step, repo, repo_path, guard_fail_on, &[])
}

/// [`run_round_block`] with extra environment, set after the sandbox scrubs
/// the repository-selecting variables.
fn run_round_block_env(
    recipe: &str,
    step: &str,
    repo: &Repo,
    repo_path: &Path,
    guard_fail_on: u32,
    envs: &[(&str, &Path)],
) -> Output {
    let ah = repo.sb.root.join("ah");
    let block = commit_block(&step_field(recipe, step, "prompt"));
    // `autodrive_launch_tools` is the real launch-side tools dir, so the
    // block runs the real commit-dir check; the merge round's
    // `autodrive_tools_dir` (git-identity.sh) is the stub below.
    let script = block
        .replace("{{repo_path}}", repo_path.to_str().expect("utf-8"))
        .replace("{{pr_number}}", "7")
        .replace(
            "{{autodrive_launch_tools}}",
            launch_tools().to_str().expect("utf-8"),
        )
        .replace(
            "{{autodrive_tools_dir}}",
            ah.join("amplifier-bundle/tools").to_str().expect("utf-8"),
        );
    assert!(
        !script.contains("{{"),
        "unsubstituted placeholder:\n{script}"
    );

    write_exec(
        &ah.join("amplifier-bundle/tools/git-identity.sh"),
        "amplihack_prepare_git_commit_identity() { :; }\n",
    );
    write_exec(
        &repo.sb.bin().join("amplihack"),
        "#!/usr/bin/env bash\n\
         printf '%s\\n' \"$*\" >> \"$STUB_AMPLIHACK_LOG\"\n\
         n=\"$(wc -l < \"$STUB_AMPLIHACK_LOG\" | tr -d ' ')\"\n\
         if [ \"$n\" = \"${STUB_GUARD_FAIL_ON:-0}\" ]; then echo \"stub guard: refusing call $n\" >&2; exit 1; fi\n\
         exit 0\n",
    );
    let mut command = Command::new("bash");
    command.arg("-c").arg(&script).current_dir(repo.sb.cwd());
    repo.sb
        .isolate(&mut command)
        .env("AMPLIHACK_HOME", &ah)
        .env("STUB_AMPLIHACK_LOG", guard_log(repo))
        .env("STUB_GUARD_FAIL_ON", guard_fail_on.to_string());
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run round commit block")
}

fn guard_log(repo: &Repo) -> PathBuf {
    repo.sb.root.join("amplihack.log")
}

fn guard_calls(repo: &Repo) -> Vec<String> {
    fs::read_to_string(guard_log(repo))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

fn assert_stopped(output: &Output, context: &str) {
    assert_ne!(
        output.status.code(),
        Some(0),
        "{context}\n{}",
        describe(output)
    );
    assert!(
        text(&output.stderr).contains("STOP"),
        "{context}: must say STOP\n{}",
        describe(output)
    );
}

#[test]
fn round_step_04_snippet_stops_outside_the_pr_worktree() {
    // SAC4 (i), (iii) and the other ways of standing in the wrong place.
    for (recipe, step) in ROUNDS {
        // (i) The main checkout, even on the PR head branch.
        let repo = repo();
        repo.sb.git(&repo.main, &["checkout", "-q", "-b", "feat/x"]);
        repo.sb.gh_head("feat/x");
        fs::write(repo.main.join("stray.txt"), "must never be staged\n").expect("write");
        let before = repo.snapshot();
        let output = run_round_block(recipe, step, &repo, &repo.main, 0);
        assert_stopped(&output, &format!("{recipe}: main checkout"));
        assert!(
            guard_calls(&repo).is_empty(),
            "{recipe}: stop before the guard"
        );
        assert_eq!(
            repo.snapshot(),
            before,
            "{recipe}: nothing staged or committed"
        );

        // (iii) A detached linked worktree, with gh failing.
        let (repo, wt) = round_fixture();
        repo.sb.git(&wt, &["checkout", "-q", "--detach"]);
        repo.sb.gh_fail();
        fs::write(wt.join("change.txt"), "work\n").expect("write");
        let head = repo.sb.git(&wt, &["rev-parse", "HEAD"]);
        let output = run_round_block(recipe, step, &repo, &wt, 0);
        assert_stopped(&output, &format!("{recipe}: detached, gh failing"));
        assert!(guard_calls(&repo).is_empty());
        assert_eq!(repo.sb.git(&wt, &["rev-parse", "HEAD"]), head);
        assert!(repo.sb.git(&wt, &["diff", "--cached", "--raw"]).is_empty());

        // A detached linked worktree where gh prints nothing: "" = "" must
        // not pass the head check (SEC-6).
        let (repo, wt) = round_fixture();
        repo.sb.git(&wt, &["checkout", "-q", "--detach"]);
        repo.sb.gh_head("");
        let output = run_round_block(recipe, step, &repo, &wt, 0);
        assert_stopped(&output, &format!("{recipe}: detached, empty PR head"));
        assert!(guard_calls(&repo).is_empty());

        // A linked worktree on a different branch than the PR head.
        let (repo, wt) = round_fixture();
        repo.sb.git(&wt, &["checkout", "-q", "-b", "feat/other"]);
        let output = run_round_block(recipe, step, &repo, &wt, 0);
        assert_stopped(&output, &format!("{recipe}: wrong branch"));
        assert!(guard_calls(&repo).is_empty());

        // A repo_path that does not exist.
        let (repo, _) = round_fixture();
        let output = run_round_block(recipe, step, &repo, &repo.sb.root.join("gone"), 0);
        assert_stopped(&output, &format!("{recipe}: missing repo_path"));
        assert!(guard_calls(&repo).is_empty());
    }
}

#[test]
fn round_step_04_snippet_commits_only_after_both_guards_pass() {
    // SAC4 (ii) and SEC-5: both guard runs are binding.
    for (recipe, step) in ROUNDS {
        // (ii) The PR's linked worktree on its head branch: commit and push.
        let (repo, wt) = round_fixture();
        fs::write(wt.join("change.txt"), "round work\n").expect("write");
        let main_before = repo.snapshot();
        let output = run_round_block(recipe, step, &repo, &wt, 0);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{recipe}\n{}",
            describe(&output)
        );
        assert_eq!(
            guard_calls(&repo),
            vec![
                "hygiene artifact-guard --repo . --mode pre-commit".to_string(),
                "hygiene artifact-guard --repo . --mode pre-commit".to_string(),
            ],
            "{recipe}: guard before and after staging"
        );
        let committed = repo
            .sb
            .git(&wt, &["show", "--name-only", "--format=", "HEAD"]);
        assert!(
            committed.lines().any(|line| line == "change.txt"),
            "{recipe}: the round's change is committed in the PR worktree:\n{committed}"
        );
        assert_eq!(
            repo.sb.git(&wt, &["rev-parse", "HEAD"]),
            repo.sb
                .git(&repo.origin, &["rev-parse", "refs/heads/feat/x"]),
            "{recipe}: and pushed to the PR branch"
        );
        assert_eq!(
            repo.snapshot(),
            main_before,
            "{recipe}: the main checkout is untouched"
        );

        // The first guard refuses: nothing is staged.
        let (repo, wt) = round_fixture();
        fs::write(wt.join("change.txt"), "round work\n").expect("write");
        let head = repo.sb.git(&wt, &["rev-parse", "HEAD"]);
        let output = run_round_block(recipe, step, &repo, &wt, 1);
        assert_stopped(&output, &format!("{recipe}: first guard refuses"));
        assert_eq!(guard_calls(&repo).len(), 1);
        assert!(
            repo.sb.git(&wt, &["diff", "--cached", "--raw"]).is_empty(),
            "{recipe}: nothing staged after the first guard refuses"
        );
        assert_eq!(repo.sb.git(&wt, &["rev-parse", "HEAD"]), head);

        // The second guard refuses: nothing is committed.
        let (repo, wt) = round_fixture();
        fs::write(wt.join("change.txt"), "round work\n").expect("write");
        let head = repo.sb.git(&wt, &["rev-parse", "HEAD"]);
        let output = run_round_block(recipe, step, &repo, &wt, 2);
        assert_stopped(&output, &format!("{recipe}: second guard refuses"));
        assert_eq!(guard_calls(&repo).len(), 2);
        assert_eq!(
            repo.sb.git(&wt, &["rev-parse", "HEAD"]),
            head,
            "{recipe}: no commit after the second guard refuses"
        );
        assert_eq!(
            repo.sb
                .git(&repo.origin, &["rev-parse", "refs/heads/feat/x"]),
            head,
            "{recipe}: nothing pushed after the second guard refuses"
        );
    }
}

// ---------------------------------------------------------------------------
// AC10: documentation
// ---------------------------------------------------------------------------

#[test]
fn auto_drive_reference_has_where_phases_2_and_3_run_section() {
    // mkdocs does not fail on a broken anchor, so pin the section and the link.
    let reference = read(&workspace_root().join("docs/reference/auto-drive-to-merge.md"));
    let heading = "\n## Where phases 2 and 3 run\n";
    let start = reference
        .find(heading)
        .expect("docs/reference/auto-drive-to-merge.md needs `## Where phases 2 and 3 run`");
    let files = reference.find("\n## Files").expect("`## Files` section");
    assert!(start < files, "the section sits before `## Files`");
    let body_start = start + heading.len();
    let section = &reference[body_start
        ..reference[body_start..]
            .find("\n## ")
            .map_or(reference.len(), |end| body_start + end)];
    for required in [
        "autodrive_pr_worktree.sh",
        "main checkout",
        "linked worktree",
        "fork",
    ] {
        assert!(
            section.contains(required),
            "the section must cover `{required}`"
        );
    }
    for line in section.lines() {
        assert!(
            !(line.contains("git rm") && line.contains(" -f")),
            "the reference section must not carry `git rm ... -f` text: {line}"
        );
    }

    let guard_doc = read(&workspace_root().join("docs/artifact-guard.md"));
    assert!(
        guard_doc.contains("reference/auto-drive-to-merge.md#where-phases-2-and-3-run"),
        "docs/artifact-guard.md must link the section"
    );
    assert!(
        guard_doc.contains("run-worktree-gitlink") && guard_doc.contains("git rm --cached -f --"),
        "docs/artifact-guard.md must document the rule and its index-only remedy"
    );
    assert!(
        !guard_doc.contains("git restore --staged -- 'worktrees/"),
        "docs/artifact-guard.md must not offer `git restore --staged` for a gitlink"
    );
}

// ---------------------------------------------------------------------------
// Security review: SEC-V1..V4 (resolver environment and verification),
// SEC-A1..A3 (fork filter, `--head`, launch-side merge gate), SEC-D2/D3.
// ---------------------------------------------------------------------------

impl Repo {
    /// Run the resolver from `cwd` with extra environment set after the
    /// sandbox scrubs the repository-selecting variables.
    fn resolve_with(&self, cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut command = Command::new("bash");
        command.arg(helper()).args(args).current_dir(cwd);
        self.sb.isolate(&mut command);
        for (key, value) in envs {
            command.env(key, value);
        }
        command.output().expect("run autodrive_pr_worktree.sh")
    }
}

/// A second repository in the same sandbox, on `main` with one commit.
fn other_repo(sb: &Sandbox, dir: &str) -> PathBuf {
    let other = sb.root.join(dir);
    fs::create_dir_all(&other).expect("mkdir other repo");
    sb.git(&other, &["init", "-q", "-b", "main"]);
    fs::write(other.join("OTHER.md"), "another repository\n").expect("write");
    sb.git(&other, &["add", "OTHER.md"]);
    sb.git(&other, &["commit", "-qm", "other"]);
    other
}

#[test]
fn helper_hardens_its_environment_before_running_git() {
    // SEC-V1/V3/D3 and SEC-A1, as written in the script.
    let script = read(&helper());
    let lines = logical_lines(&script);
    for required in ["set -euo pipefail", "export LC_ALL=C"] {
        assert!(
            lines.iter().any(|line| line == required),
            "helper must run `{required}`"
        );
    }
    let unset_lines: Vec<(usize, &String)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("unset "))
        .collect();
    let unset_words: Vec<&str> = unset_lines
        .iter()
        .flat_map(|(_, line)| line.split_whitespace().skip(1))
        .collect();
    for var in amplihack_git::REPOSITORY_ENV_VARS
        .iter()
        .copied()
        .chain(["CDPATH", "GH_REPO"])
    {
        assert!(
            unset_words.contains(&var),
            "helper must `unset {var}` (amplihack_git::REPOSITORY_ENV_VARS, CDPATH, GH_REPO; SEC-V1/V3)"
        );
    }
    let last_unset = unset_lines.iter().map(|(at, _)| *at).max().expect("unset");
    let first_git = lines
        .iter()
        .position(|line| !git_invocations(line).is_empty())
        .expect("helper runs git");
    assert!(
        last_unset < first_git,
        "the environment is cleaned before the first git call"
    );

    let fetch = lines
        .iter()
        .find(|line| {
            git_invocations(line)
                .iter()
                .any(|words| subcommand(words).is_some_and(|(sub, _)| sub == "fetch"))
        })
        .expect("helper fetches the branch");
    for required in [
        "GIT_TERMINAL_PROMPT=0",
        "--no-tags",
        "origin",
        "+refs/heads/",
        ":refs/remotes/origin/",
    ] {
        assert!(
            fetch.contains(required),
            "the fetch must contain `{required}` (SEC-D3): {fetch}"
        );
    }

    assert!(
        script.contains("--json headRefName,isCrossRepository"),
        "gh is asked for the head branch and the fork flag together"
    );
    assert!(
        script.contains("select(.isCrossRepository == false) | .headRefName"),
        "SEC-A1: a missing or null fork flag must count as a fork"
    );
    assert!(
        !script.contains("isCrossRepository | not") && !script.contains("isCrossRepository|not"),
        "SEC-A1: `| not` reads a null fork flag as same-repository"
    );
}

#[test]
fn refuses_a_pr_whose_fork_flag_is_missing_or_null() {
    // SEC-A1 behaviour: `null` is not `false`.
    let repo = repo();
    repo.push_from_elsewhere("feat/x");
    repo.sb.gh_head("feat/x");
    repo.sb.gh_cross_null();
    let before = repo.snapshot();
    let records = repo.worktree_records();

    assert_refused(&repo.resolve_pr("7"), 1, "fork flag missing or null");

    assert_eq!(repo.worktree_records(), records);
    assert!(!repo.has_ref("refs/heads/feat/x"), "nothing created");
    assert!(
        !repo.has_ref("refs/remotes/origin/feat/x"),
        "nothing fetched"
    );
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn dash_prefixed_values_and_repeated_flags_are_usage_errors() {
    // SEC-V2: a value that starts with `-` counts as missing, so `--repo --pr 5`
    // is an error, not a repository named `--pr`.
    let repo = repo();
    let main = repo.main_str().to_string();
    let before = repo.snapshot();
    let records = repo.worktree_records();
    let cases: Vec<Vec<&str>> = vec![
        vec!["resolve", "--repo", "-", "--head", "feat/x"],
        vec!["resolve", "--repo", "--pr", "5"],
        vec!["resolve", "--repo", "--head", "feat/x"],
        vec!["resolve", "--repo", &main, "--pr", "--head", "feat/x"],
        vec!["resolve", "--repo", &main, "--head", "--pr", "5"],
        vec![
            "resolve", "--repo", &main, "--repo", &main, "--head", "feat/x",
        ],
        vec!["resolve", "--repo", &main, "--pr", "5", "--pr", "5"],
        vec![
            "resolve", "--repo", &main, "--head", "feat/x", "--head", "feat/x",
        ],
    ];
    for args in cases {
        assert_refused(&repo.resolve(&args), 2, &format!("usage {args:?}"));
    }
    assert!(
        repo.sb.gh_calls().is_empty(),
        "usage errors are caught before gh runs:\n{}",
        repo.sb.gh_calls()
    );
    assert_eq!(repo.worktree_records(), records);
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn relative_repo_resolves_from_the_current_directory_never_through_cdpath() {
    // SEC-V1: with CDPATH set, `cd main` would enter `<decoy>/main` and print
    // its path on stdout, corrupting the one line the caller captures.
    let repo = repo();
    repo.local_branch("feat/x");
    let side = repo.sb.root.join("side/x");
    repo.add_worktree(&side, "feat/x");

    let decoy_parent = repo.sb.root.join("decoy");
    let decoy = other_repo(&repo.sb, "decoy/main");
    repo.sb.git(&decoy, &["branch", "feat/x"]);
    let decoy_side = repo.sb.root.join("decoy-side/x");
    repo.sb.git(
        &decoy,
        &[
            "worktree",
            "add",
            "-q",
            decoy_side.to_str().expect("utf-8"),
            "feat/x",
        ],
    );
    let before = repo.snapshot();

    let output = repo.resolve_with(
        &repo.sb.root,
        &["resolve", "--repo", "main", "--head", "feat/x"],
        &[("CDPATH", decoy_parent.to_str().expect("utf-8"))],
    );

    assert_eq!(
        resolved_path(&output, "relative --repo with CDPATH set"),
        canonical(&side),
        "the relative --repo must resolve from the current directory"
    );
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn inherited_repository_variables_cannot_redirect_the_resolver() {
    // SEC-V3: an exported GIT_DIR, GIT_WORK_TREE, GIT_COMMON_DIR or
    // GIT_INDEX_FILE would point every git call at another repository. The
    // resolver must still fetch and create the worktree in --repo, and leave
    // the other repository exactly as it was.
    let repo = repo();
    repo.push_from_elsewhere("feat/x");
    let other = other_repo(&repo.sb, "other");
    let other_git = other.join(".git");
    let stray_index = repo.sb.root.join("stray-index");
    let other_before = (
        repo.sb.git(&other, &["worktree", "list", "--porcelain"]),
        repo.sb.git(&other, &["for-each-ref"]),
        repo.sb.git(&other, &["ls-files", "-s"]),
    );
    let before = repo.snapshot();

    let output = repo.resolve_with(
        &repo.sb.cwd(),
        &["resolve", "--repo", repo.main_str(), "--head", "feat/x"],
        &[
            ("GIT_DIR", other_git.to_str().expect("utf-8")),
            ("GIT_COMMON_DIR", other_git.to_str().expect("utf-8")),
            ("GIT_WORK_TREE", other.to_str().expect("utf-8")),
            ("GIT_INDEX_FILE", stray_index.to_str().expect("utf-8")),
        ],
    );

    let path = resolved_path(&output, "inherited GIT_DIR");
    assert_eq!(path, canonical(&repo.main.join("worktrees/feat/x")));
    assert_eq!(
        repo.sb
            .git(Path::new(&path), &["branch", "--show-current"])
            .trim(),
        "feat/x"
    );
    assert_eq!(
        (
            repo.sb.git(&other, &["worktree", "list", "--porcelain"]),
            repo.sb.git(&other, &["for-each-ref"]),
            repo.sb.git(&other, &["ls-files", "-s"]),
        ),
        other_before,
        "the other repository must be untouched"
    );
    assert!(!stray_index.exists(), "no index written at GIT_INDEX_FILE");
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refuses_a_registered_path_now_holding_another_repositorys_worktree() {
    // SEC-V4: the registered worktree directory was replaced by a worktree of
    // a different repository on a same-named branch. Git still lists the
    // record (it is not prunable: the `.git` file exists) and its HEAD is
    // `refs/heads/feat/x`. Only the common-dir comparison tells them apart.
    let repo = repo();
    repo.local_branch("feat/x");
    let side = repo.sb.root.join("side/x");
    repo.add_worktree(&side, "feat/x");
    let foreign = other_repo(&repo.sb, "foreign");
    repo.sb.git(&foreign, &["branch", "feat/x"]);
    fs::remove_dir_all(&side).expect("remove registered worktree dir");
    repo.sb.git(
        &foreign,
        &[
            "worktree",
            "add",
            "-q",
            side.to_str().expect("utf-8"),
            "feat/x",
        ],
    );
    let listing = repo
        .sb
        .git(&repo.main, &["worktree", "list", "--porcelain"]);
    assert!(
        listing.contains(&format!("worktree {}\n", canonical(&side)))
            && listing.contains("branch refs/heads/feat/x")
            && !listing.contains("prunable"),
        "fixture: the main checkout still lists side/x on feat/x:\n{listing}"
    );
    let foreign_state = (
        repo.sb.git(&side, &["rev-parse", "HEAD"]),
        repo.sb.git(&side, &["status", "--porcelain"]),
    );
    let before = repo.snapshot();
    let records = repo.worktree_records();

    assert_refused(
        &repo.resolve_head("feat/x"),
        1,
        "foreign worktree at a registered path",
    );

    assert_eq!(
        (
            repo.sb.git(&side, &["rev-parse", "HEAD"]),
            repo.sb.git(&side, &["status", "--porcelain"]),
        ),
        foreign_state,
        "the foreign worktree is untouched"
    );
    assert_eq!(repo.worktree_records(), records);
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refusals_never_echo_the_rejected_branch_name() {
    // The branch name is attacker-chosen; the refusal names the problem, not
    // the value.
    let repo = repo();
    for head in ["feat/zqx$(id)", "feat;zqx", "feat/zqx`id`", "feat/zqx\nb"] {
        let output = repo.resolve_head(head);
        assert_refused(&output, 1, &format!("--head {head:?}"));
        assert!(
            !text(&output.stderr).contains("zqx"),
            "--head {head:?}: the rejected value must not be echoed\n{}",
            describe(&output)
        );
    }
    repo.sb.gh_head("feat/zqx$(id)");
    let output = repo.resolve_pr("7");
    assert_refused(&output, 1, "gh head with metacharacters");
    assert!(
        !text(&output.stderr).contains("zqx"),
        "a gh-supplied head must not be echoed\n{}",
        describe(&output)
    );
}

#[test]
fn head_mode_never_asks_gh() {
    // D10 / SEC-A2: `--head` is for an operator; it makes no gh call.
    let repo = repo();
    repo.local_branch("feat/x");
    let side = repo.sb.root.join("side/x");
    repo.add_worktree(&side, "feat/x");

    resolved_path(&repo.resolve_head("feat/x"), "--head");

    assert!(
        repo.sb.gh_calls().is_empty(),
        "--head must not call gh:\n{}",
        repo.sb.gh_calls()
    );
}

#[test]
fn loops_resolve_by_pr_only_and_keep_the_merge_gate_on_the_launch_side() {
    // SEC-A2: the loops never pass `--head`. SEC-A3: the merge gate keeps
    // `--repo "${REPO_PATH:-.}"` and never sees the PR worktree.
    for (recipe, step, _) in LOOPS {
        let command = step_field(recipe, step, "command");
        let resolve_line = logical_line_at(
            &command,
            command
                .find("autodrive_pr_worktree.sh")
                .expect("step-02 resolves the PR worktree"),
        );
        assert!(
            !resolve_line.contains("--head"),
            "{recipe}: the loop resolves by --pr only:\n{resolve_line}"
        );
        assert!(
            resolve_line.contains("$(dirname \"$LOOP_HELPER\")/autodrive_pr_worktree.sh"),
            "{recipe}: the resolver is found next to the launch-side loop helper:\n{resolve_line}"
        );
    }
    let gate = step_field("autodrive-merge-loop.yaml", "step-03-merge-gate", "command");
    assert!(
        gate.contains("--repo \"${REPO_PATH:-.}\""),
        "the merge gate keeps --repo \"${{REPO_PATH:-.}}\":\n{gate}"
    );
    assert!(
        !gate.contains("PR_WORKTREE") && !gate.contains("repo_path"),
        "the merge gate never runs against the PR worktree:\n{gate}"
    );
}

#[test]
fn round_step_04_blocks_unset_inherited_repository_variables_after_cd() {
    // SEC-D2, as written.
    for (recipe, step) in ROUNDS {
        let lines = logical_lines(&commit_block(&step_field(recipe, step, "prompt")));
        assert_eq!(
            lines.get(1).map(String::as_str),
            Some("unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GH_REPO"),
            "{recipe} {step}: the line right after `cd` unsets the repository variables:\n{}",
            lines.join("\n")
        );
    }
}

#[test]
fn round_step_04_commits_in_the_pr_worktree_despite_an_inherited_git_dir_and_index() {
    // SEC-D2, run: an inherited GIT_DIR/GIT_WORK_TREE would make the main
    // checkout look like the current repository, and GIT_INDEX_FILE would make
    // `git add -A` and `git commit` use an index the guard never read.
    for (recipe, step) in ROUNDS {
        let (repo, wt) = round_fixture();
        fs::write(wt.join("change.txt"), "round work\n").expect("write");
        let stray_index = repo.sb.root.join("stray-index");
        let main_git = repo.main.join(".git");
        let main_before = repo.snapshot();

        let output = run_round_block_env(
            recipe,
            step,
            &repo,
            &wt,
            0,
            &[
                ("GIT_DIR", &main_git),
                ("GIT_WORK_TREE", &repo.main),
                ("GIT_INDEX_FILE", &stray_index),
            ],
        );

        assert_eq!(
            output.status.code(),
            Some(0),
            "{recipe}\n{}",
            describe(&output)
        );
        let committed = repo
            .sb
            .git(&wt, &["show", "--name-only", "--format=", "HEAD"]);
        assert!(
            committed.lines().any(|line| line == "change.txt"),
            "{recipe}: the change is committed in the PR worktree:\n{committed}"
        );
        assert!(!stray_index.exists(), "{recipe}: no stray index written");
        assert_eq!(
            repo.snapshot(),
            main_before,
            "{recipe}: main checkout untouched"
        );
    }
}

// ---------------------------------------------------------------------------
// Currency against origin: a resumed run never reviews a stale tip (#1528)
// ---------------------------------------------------------------------------

impl Repo {
    fn origin_tip(&self, branch: &str) -> String {
        self.sb.git(
            &self.origin,
            &["rev-parse", &format!("refs/heads/{branch}")],
        )
    }

    fn local_tip(&self, branch: &str) -> String {
        self.sb
            .git(&self.main, &["rev-parse", &format!("refs/heads/{branch}")])
    }

    /// Add a commit to the local `branch` without checking it out anywhere.
    fn commit_on_local(&self, branch: &str, message: &str) {
        let parent = self.local_tip(branch);
        let parent = parent.trim();
        let tree = self
            .sb
            .git(&self.main, &["rev-parse", &format!("{parent}^{{tree}}")]);
        let commit = self.sb.git(
            &self.main,
            &["commit-tree", tree.trim(), "-p", parent, "-m", message],
        );
        self.sb.git(
            &self.main,
            &[
                "update-ref",
                &format!("refs/heads/{branch}"),
                commit.trim(),
                parent,
            ],
        );
    }
}

#[test]
fn fast_forwards_a_local_branch_that_is_behind_origin() {
    // The resume case: the phase-1 worktree is gone, the local branch was left
    // behind at `main`, and the PR has since moved on origin.
    let repo = repo();
    repo.local_branch("feat/x");
    repo.push_from_elsewhere("feat/x");
    assert_ne!(repo.local_tip("feat/x"), repo.origin_tip("feat/x"));
    let before = repo.snapshot();

    let path = resolved_path(&repo.resolve_head("feat/x"), "local branch behind origin");

    assert_eq!(path, canonical(&repo.main.join("worktrees/feat/x")));
    assert_eq!(
        repo.sb.git(Path::new(&path), &["rev-parse", "HEAD"]),
        repo.origin_tip("feat/x"),
        "the new worktree must be at origin's tip, not the stale local one"
    );
    assert_eq!(repo.local_tip("feat/x"), repo.origin_tip("feat/x"));
    assert_eq!(repo.snapshot(), before, "main checkout untouched");
}

#[test]
fn keeps_a_local_branch_that_is_ahead_of_origin() {
    // Unpushed local work is never rewound.
    let repo = repo();
    repo.push_from_elsewhere("feat/x");
    repo.sb
        .git(&repo.main, &["fetch", "-q", "origin", "feat/x:feat/x"]);
    repo.commit_on_local("feat/x", "unpushed");
    let local = repo.local_tip("feat/x");
    assert_ne!(local, repo.origin_tip("feat/x"));

    let path = resolved_path(&repo.resolve_head("feat/x"), "local branch ahead of origin");

    assert_eq!(repo.sb.git(Path::new(&path), &["rev-parse", "HEAD"]), local);
    assert_eq!(
        repo.local_tip("feat/x"),
        local,
        "the local branch is not rewound"
    );
}

#[test]
fn refuses_a_local_branch_that_has_diverged_from_origin() {
    let repo = repo();
    repo.local_branch("feat/x");
    repo.commit_on_local("feat/x", "local only");
    repo.push_from_elsewhere("feat/x");
    let local = repo.local_tip("feat/x");
    let before = repo.snapshot();
    let records = repo.worktree_records();

    let output = repo.resolve_head("feat/x");
    assert_refused(&output, 1, "diverged local branch");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("diverged") && stderr.contains("#1528"),
        "the refusal must say the branch has diverged:\n{stderr}"
    );

    assert_eq!(
        repo.local_tip("feat/x"),
        local,
        "the local branch is untouched"
    );
    assert_eq!(repo.worktree_records(), records, "no worktree created");
    assert!(!repo.main.join("worktrees/feat/x").exists());
    assert_eq!(repo.snapshot(), before);
}

#[test]
fn refuses_an_existing_worktree_that_is_behind_origin() {
    // The matched worktree is never updated in place; the operator is told.
    let repo = repo();
    repo.local_branch("feat/x");
    let side = repo.sb.root.join("side/x");
    repo.add_worktree(&side, "feat/x");
    repo.push_from_elsewhere("feat/x");
    let head = repo.sb.git(&side, &["rev-parse", "HEAD"]);
    let before = repo.snapshot();

    let output = repo.resolve_head("feat/x");
    assert_refused(&output, 1, "existing worktree behind origin");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("behind") && stderr.contains("#1528"),
        "the refusal must say the worktree is behind origin:\n{stderr}"
    );

    assert_eq!(
        repo.sb.git(&side, &["rev-parse", "HEAD"]),
        head,
        "worktree untouched"
    );
    assert_eq!(repo.snapshot(), before);

    // Once the operator fast-forwards it, the same worktree resolves.
    repo.sb
        .git(&side, &["merge", "-q", "--ff-only", "origin/feat/x"]);
    let path = resolved_path(&repo.resolve_head("feat/x"), "after fast-forward");
    assert_eq!(path, canonical(&side));
}

#[test]
fn refuses_when_origin_cannot_be_asked() {
    // Currency cannot be checked, so the resolver stops rather than guess.
    let repo = repo();
    repo.local_branch("feat/x");
    let side = repo.sb.root.join("side/x");
    repo.add_worktree(&side, "feat/x");
    let missing = repo.sb.root.join("no-such-origin.git");
    repo.sb.git(
        &repo.main,
        &[
            "remote",
            "set-url",
            "origin",
            missing.to_str().expect("utf-8"),
        ],
    );

    assert_refused(&repo.resolve_head("feat/x"), 1, "unreachable origin");
}
