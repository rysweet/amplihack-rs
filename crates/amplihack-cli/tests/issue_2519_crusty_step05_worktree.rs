//! Issue #2519: step-05 of `autodrive-crusty-round.yaml` measures the head
//! and the commit count in the PR's linked worktree, the folder step-04
//! commits in, and never in the folder the step happened to start in.
//!
//! The step-05 body is extracted from the live YAML and run in bash as
//! `cd <start> && <body>`. The runner's `working_dir` is deliberately not
//! simulated: the body's own `cd` into `repo_path` must do the work. The real
//! `autodrive_worktree_guard.sh commit-dir` check runs against a real linked
//! worktree (`git worktree add ... -b quick/demo`), with the fake `gh`
//! answering `headRefName` `quick/demo` for PR 7.
//!
//! HOME is a temporary folder, PATH holds the fakes ahead of the system
//! directories, and nothing reaches the network. Every test skips, with a
//! note, when `jq` is not installed.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};

use crate::support::auto_drive_light::{
    Fakes, LightRepo, fake_bin, git, jq_available, run_bash, show, stderr, stdout, step_command,
    tools_dir,
};

const ROUND: &str = "autodrive-crusty-round";
const STEP05: &str = "step-05-verify-concerns-addressed";
const PR: &str = "7";

fn skip() -> bool {
    if !jq_available() {
        eprintln!("skip: `jq` not available on PATH");
        return true;
    }
    false
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|e| panic!("canonicalize {}: {e}", p.display()))
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A clone (`root`, the main checkout on `main`) with a linked worktree
/// `worktrees/pr-7` on `quick/demo`, the PR head branch the fake `gh` names.
struct Fixture {
    repo: LightRepo,
    root: PathBuf,
    worktree: PathBuf,
    fakes: Fakes,
    home: PathBuf,
    /// The reviewed head: the worktree's HEAD when the round started.
    h0: String,
}

impl Fixture {
    fn new() -> Self {
        let repo = LightRepo::new();
        let worktree_path = repo.tmp.path().join("worktrees").join("pr-7");
        git(
            &repo.wt,
            &[
                "worktree",
                "add",
                "-q",
                worktree_path.to_str().unwrap(),
                "-b",
                "quick/demo",
            ],
        );
        let root = canon(&repo.wt);
        let worktree = canon(&worktree_path);
        let h0 = git(&worktree, &["rev-parse", "HEAD"]);
        let fakes = fake_bin(repo.tmp.path());
        let home = repo.home.clone();
        Fixture {
            repo,
            root,
            worktree,
            fakes,
            home,
            h0,
        }
    }

    /// step-04's commit: a change committed in the PR worktree.
    fn commit_in_worktree(&self) -> String {
        std::fs::write(self.worktree.join("docs/fix.md"), "# fix\n").unwrap();
        git(&self.worktree, &["add", "-A"]);
        git(&self.worktree, &["commit", "-q", "-m", "address concerns"]);
        git(&self.worktree, &["rev-parse", "HEAD"])
    }

    fn context(&self, head_sha: &str) -> String {
        json!({ "pr": PR, "head_sha": head_sha }).to_string()
    }

    /// Runs `cd <start> && <step-05 body>` with `repo_path`, `pr_number`,
    /// the round context and `REPO_PATH` as given.
    fn run(&self, start: &Path, repo_path: &str, head_sha: &str, repo_path_env: &str) -> Output {
        self.run_with(start, repo_path, head_sha, repo_path_env, &[])
    }

    /// As `run`, with `extra` added to the inherited environment.
    fn run_with(
        &self,
        start: &Path,
        repo_path: &str,
        head_sha: &str,
        repo_path_env: &str,
        extra: &[(&str, &OsStr)],
    ) -> Output {
        let body = step_command(ROUND, STEP05);
        let script = format!("cd {} && {}", shell_quote(&s(start)), body);
        let tools = tools_dir();
        let ctx = self.context(head_sha);
        let path = std::env::join_paths(
            std::iter::once(self.fakes.bin.clone()).chain(
                ["/usr/local/bin", "/usr/bin", "/bin"]
                    .iter()
                    .map(PathBuf::from),
            ),
        )
        .unwrap();
        let amplihack_home = self.repo.tmp.path().join("amplihack-home");
        std::fs::create_dir_all(&amplihack_home).unwrap();
        let mut env: Vec<(&str, &OsStr)> = vec![
            ("PATH", path.as_os_str()),
            ("AMPLIHACK_HOME", amplihack_home.as_os_str()),
            ("AUTODRIVE_LAUNCH_TOOLS", tools.as_os_str()),
            ("RECIPE_VAR_repo_path", OsStr::new(repo_path)),
            ("RECIPE_VAR_pr_number", OsStr::new(PR)),
            ("RECIPE_VAR_crusty_round_context", OsStr::new(&ctx)),
            ("REPO_PATH", OsStr::new(repo_path_env)),
        ];
        env.extend_from_slice(extra);
        run_bash(&self.home, &self.fakes, Some(&script), &[], &env, "")
    }
}

fn shell_quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', r"'\''"))
}

fn evidence(o: &Output) -> Value {
    let out = stdout(o);
    let line = out
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("step-05 printed no JSON\n{}", show(o)));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("bad JSON {line:?}: {e}\n{}", show(o)))
}

/// A refusal: non-zero exit, a `STOP: step-05` line naming #2519 and #1528,
/// and nothing on stdout for `parse_json` to read as `commits: 0`.
fn assert_stopped(o: &Output) -> String {
    assert!(
        !o.status.success(),
        "step-05 must stop, not report\n{}",
        show(o)
    );
    let err = stderr(o);
    let line = err
        .lines()
        .find(|l| l.contains("STOP: step-05"))
        .unwrap_or_else(|| panic!("no `STOP: step-05` line\n{}", show(o)))
        .to_string();
    assert!(
        line.contains("(#2519, #1528)"),
        "STOP line must name #2519 and #1528: {line}"
    );
    let out = stdout(o);
    assert!(
        !out.contains("\"commits\"") && out.trim().is_empty(),
        "a stopped step-05 writes nothing to stdout\n{}",
        show(o)
    );
    line
}

// --- Case 1: step-04 committed in the PR worktree; start is the clone root ---

#[test]
fn step05_reports_the_commit_step04_made_in_the_pr_worktree() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let h1 = f.commit_in_worktree();
    assert_ne!(h1, f.h0);
    // Started in the clone root, with REPO_PATH also naming the clone root:
    // the old body measured there and reported 0 commits.
    let o = f.run(&f.root, &s(&f.worktree), &f.h0, &s(&f.root));
    assert!(o.status.success(), "{}", show(&o));
    let v = evidence(&o);
    assert_eq!(v["head_sha"], json!(h1), "{}", show(&o));
    assert_eq!(v["base_sha"], json!(f.h0), "{}", show(&o));
    assert!(
        v["commits"].as_u64().unwrap_or(0) >= 1,
        "commits must count step-04's commit\n{}",
        show(&o)
    );
    assert_eq!(v["git_observed"], json!("true"), "{}", show(&o));
    assert_eq!(v["dirty_worktree"], json!("false"), "{}", show(&o));
    assert!(
        !stderr(&o).contains("NO commit"),
        "a round with a commit must not warn\n{}",
        show(&o)
    );
}

#[test]
fn step05_keeps_the_zero_commit_warning_for_a_measured_zero() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let o = f.run(&f.root, &s(&f.worktree), &f.h0, &s(&f.root));
    assert!(o.status.success(), "{}", show(&o));
    let v = evidence(&o);
    assert_eq!(v["commits"], json!(0), "{}", show(&o));
    assert_eq!(v["head_sha"], json!(f.h0), "{}", show(&o));
    assert_eq!(v["git_observed"], json!("true"), "{}", show(&o));
    assert!(stderr(&o).contains("NO commit"), "{}", show(&o));
}

#[test]
fn step05_reports_a_dirty_pr_worktree() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    f.commit_in_worktree();
    std::fs::write(f.worktree.join("README.md"), "# changed, not committed\n").unwrap();
    let o = f.run(&f.root, &s(&f.worktree), &f.h0, &s(&f.root));
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(
        evidence(&o)["dirty_worktree"],
        json!("true"),
        "{}",
        show(&o)
    );
}

/// An inherited `GIT_COMMON_DIR` naming another repository (as under a git
/// hook) must not move step-05's own git calls away from the worktree the
/// guard checked: step-05 clears it through `git-repo-env.sh`.
#[test]
fn step05_measures_the_pr_worktree_despite_an_inherited_git_common_dir() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let h1 = f.commit_in_worktree();
    let other = f.repo.tmp.path().join("other-repo");
    std::fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q"]);
    std::fs::write(other.join("x.txt"), "other\n").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-q", "-m", "other"]);
    let other_git = canon(&other.join(".git"));
    let o = f.run_with(
        &f.root,
        &s(&f.worktree),
        &f.h0,
        &s(&f.root),
        &[("GIT_COMMON_DIR", other_git.as_os_str())],
    );
    assert!(o.status.success(), "{}", show(&o));
    let v = evidence(&o);
    assert_eq!(v["head_sha"], json!(h1), "{}", show(&o));
    assert_eq!(v["base_sha"], json!(f.h0), "{}", show(&o));
    assert_eq!(v["commits"], json!(1), "{}", show(&o));
}

// --- Case 2: repo_path is not the PR worktree; step-05 stops ---

#[test]
fn step05_stops_when_repo_path_is_the_main_checkout() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    f.commit_in_worktree();
    // Started in the PR worktree and REPO_PATH names it too: neither may be
    // used in place of repo_path.
    let o = f.run(&f.worktree, &s(&f.root), &f.h0, &s(&f.worktree));
    let line = assert_stopped(&o);
    let expected = format!(
        "STOP: step-05 repo_path {} is not PR #7's linked worktree on its head branch \
         (step started in {}); commit evidence is not measured in any other checkout (#2519, #1528)",
        s(&f.root),
        s(&f.worktree)
    );
    assert_eq!(
        line.trim(),
        expected,
        "the STOP text is the one docs/reference/auto-drive-to-merge.md documents"
    );
}

#[test]
fn step05_stops_when_the_worktree_is_on_another_branch() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    git(&f.worktree, &["checkout", "-q", "-b", "other/branch"]);
    let o = f.run(&f.root, &s(&f.worktree), &f.h0, &s(&f.root));
    let line = assert_stopped(&o);
    assert!(line.contains(&s(&f.worktree)), "{line}");
    assert!(line.contains(&s(&f.root)), "{line}");
}

#[test]
fn step05_stops_when_repo_path_does_not_exist() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let absent = canon(f.repo.tmp.path()).join("absent");
    let o = f.run(&f.root, &s(&absent), &f.h0, &s(&f.worktree));
    let line = assert_stopped(&o);
    assert!(line.contains(&s(&absent)), "STOP names repo_path: {line}");
    assert!(
        line.contains(&s(&f.root)),
        "STOP names the start folder: {line}"
    );
}

#[test]
fn step05_stops_when_repo_path_is_empty() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    f.commit_in_worktree();
    // Started in the PR worktree itself: an empty repo_path must not fall
    // back to it (`cd ""` would succeed and stay put).
    let o = f.run(&f.worktree, "", &f.h0, &s(&f.worktree));
    assert_stopped(&o);
}

// --- head_sha from the round context is data, not a git expression ---

#[test]
fn step05_stops_on_a_head_sha_that_is_not_hex() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    // `HEAD` resolves in git, but is not a recorded commit id.
    let o = f.run(&f.root, &s(&f.worktree), "HEAD", &s(&f.root));
    assert_stopped(&o);
}

#[test]
fn step05_stops_on_a_hex_head_sha_that_is_not_a_commit_here() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let missing = "0123456789abcdef0123456789abcdef01234567";
    let o = f.run(&f.root, &s(&f.worktree), missing, &s(&f.root));
    assert_stopped(&o);
}

// --- The step text itself ---

#[test]
fn step05_runs_in_repo_path_with_the_commit_dir_guard() {
    let doc = crate::support::auto_drive_light::recipe(ROUND);
    let step = doc["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s["id"].as_str() == Some(STEP05))
        .expect("step-05 exists");
    assert_eq!(
        step["working_dir"].as_str(),
        Some("{{repo_path}}"),
        "step-05 runs in the PR worktree"
    );
    let body = step["command"].as_str().unwrap();
    assert!(
        body.contains("autodrive_worktree_guard.sh") && body.contains("commit-dir"),
        "step-05 runs the commit-dir guard"
    );
}
