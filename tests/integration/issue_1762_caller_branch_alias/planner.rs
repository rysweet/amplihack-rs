//! The planner, `workflow_caller_branch_alias.sh` (#1762 contract C2, D3,
//! D5, S5 to S7), run on its own: its exit codes, its answer, and that it
//! writes nothing.

use std::fs;

use super::fixture::*;

const USAGE: &str =
    "usage: workflow_caller_branch_alias.sh <caller_path> <main_repo_path> <branch>";

/// Everything the planner could write, in the caller's repository and on
/// origin.
#[derive(Debug, PartialEq, Eq)]
struct RepoState {
    refs: String,
    config: String,
    worktrees: String,
    stash: String,
    index: Vec<u8>,
    worktrees_dir: Vec<String>,
    origin: Vec<String>,
}

fn repo_state(fx: &Fx) -> RepoState {
    let git = |args: &[&str]| fx.git_output(&fx.caller, args).stdout;
    let mut worktrees_dir: Vec<String> = fs::read_dir(fx.caller.join("worktrees"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    worktrees_dir.sort();
    RepoState {
        refs: git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
        config: git(&["config", "--local", "--list"]),
        worktrees: git(&["worktree", "list", "--porcelain"]),
        stash: git(&["stash", "list"]),
        index: fs::read(fx.caller.join(".git/index")).unwrap_or_default(),
        worktrees_dir,
        origin: fx.origin_refs(),
    }
}

fn plan(fx: &Fx, branch: &str, envs: &[(&str, &str)]) -> Run {
    let caller = fx.caller.to_str().unwrap();
    fx.tool(PLANNER, &fx.caller, &[caller, caller, branch], envs)
}

fn assert_plans(fx: &Fx, run: &Run, expected: &str, why: &str) {
    assert_eq!(run.code, Some(0), "{why}\n{}", run.describe());
    assert_eq!(
        run.stdout,
        format!("{expected}\n"),
        "{why}: the alias alone on stdout"
    );
    let caller = fx.caller.to_str().unwrap();
    let upstream = format!("origin/{BRANCH}");
    assert!(
        run.stderr_line_with(&["INFO:", expected, &upstream, caller, "(issue #1762)."])
            .is_some(),
        "{why}: the R8 INFO line on stderr\n{}",
        run.describe()
    );
}

#[test]
fn planner_contract() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let caller = fx.caller.to_str().unwrap();

    let wrong_counts: [&[&str]; 3] = [&[], &[caller, caller], &[caller, caller, BRANCH, "extra"]];
    for args in wrong_counts {
        let run = fx.tool(PLANNER, &fx.caller, args, &[]);
        assert_eq!(
            run.code,
            Some(2),
            "{} argument(s)\n{}",
            args.len(),
            run.describe()
        );
        assert!(run.stdout.is_empty(), "{}", run.describe());
        assert!(run.stderr.contains(USAGE), "{}", run.describe());
    }
    let missing = fx.root.join("missing");
    let run = fx.tool(
        PLANNER,
        &fx.caller,
        &[missing.to_str().unwrap(), caller, BRANCH],
        &[],
    );
    assert_eq!(
        run.code,
        Some(2),
        "a missing caller directory\n{}",
        run.describe()
    );
    assert!(run.stdout.is_empty(), "{}", run.describe());

    let run = plan(&fx, "main", &[]);
    assert_eq!(
        run.code,
        Some(3),
        "the default branch gets no alias\n{}",
        run.describe()
    );
    assert!(run.stdout.is_empty(), "{}", run.describe());
    assert!(run.stderr.contains("INFO:"), "{}", run.describe());

    assert_plans(
        &fx,
        &plan(&fx, BRANCH, &[]),
        &alias(1),
        "a clean, pushed caller",
    );
}

/// The planner only reads (D3): fresh, reuse and skip cases leave refs,
/// configuration, worktrees, the caller's index and origin exactly as they
/// were.
#[test]
fn planner_writes_nothing() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let before = repo_state(&fx);
    assert_plans(&fx, &plan(&fx, BRANCH, &[]), &alias(1), "fresh");
    assert_eq!(
        repo_state(&fx),
        before,
        "fresh: the planner wrote something"
    );

    fx.alias_worktree(&alias(1), BRANCH);
    let before = repo_state(&fx);
    assert_plans(&fx, &plan(&fx, BRANCH, &[]), &alias(1), "reuse");
    assert_eq!(
        repo_state(&fx),
        before,
        "reuse: the planner wrote something"
    );

    let fx = Fx::new();
    fx.on_pushed_branch();
    fx.git(
        &fx.caller,
        &["branch", "-q", "--track", &alias(1), "origin/main"],
    );
    let before = repo_state(&fx);
    assert_plans(&fx, &plan(&fx, BRANCH, &[]), &alias(2), "skip");
    assert_eq!(repo_state(&fx), before, "skip: the planner wrote something");
}

/// S5: `@{-1}` names the previous branch; `check-ref-format --branch` expands
/// it, so the output differs from the input and the planner refuses.
#[test]
fn planner_rejects_previous_branch_syntax() {
    let fx = Fx::new();
    fx.on_pushed_branch(); // main, then BRANCH: @{-1} is main.
    for branch in ["@{-1}", "-x"] {
        let run = plan(&fx, branch, &[]);
        assert_eq!(run.code, Some(2), "{branch}\n{}", run.describe());
        assert!(run.stdout.is_empty(), "{branch}\n{}", run.describe());
        assert!(run.stderr.contains(USAGE), "{branch}\n{}", run.describe());
    }
}

/// S6: a `git status` that fails is not an empty, clean status.
#[test]
fn planner_status_unreadable_fails_closed() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    fs::write(fx.caller.join(".git/index"), b"not an index").unwrap();

    let run = plan(&fx, BRANCH, &[]);

    assert_eq!(run.code, Some(1), "{}", run.describe());
    assert!(
        run.stdout.is_empty(),
        "no alias from an unreadable status\n{}",
        run.describe()
    );
    assert!(
        run.stderr
            .lines()
            .any(|l| l.starts_with("ERROR:") && l.ends_with("(issue #1762).")),
        "{}",
        run.describe()
    );
    assert!(!run.stderr.contains("INFO:"), "{}", run.describe());
}

/// S7: an inherited GIT_DIR, GIT_WORK_TREE or GIT_INDEX_FILE cannot redirect
/// the checks to another repository. The other repository is dirty and has
/// no origin; the caller is clean and pushed.
#[test]
fn planner_ignores_inherited_git_dir() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let other = fx.other_repo("other", "main");
    fs::write(other.join("o.txt"), "dirty\n").unwrap();
    let git_dir = other.join(".git");
    let index = git_dir.join("index");

    let run = plan(
        &fx,
        BRANCH,
        &[
            ("GIT_DIR", git_dir.to_str().unwrap()),
            ("GIT_WORK_TREE", other.to_str().unwrap()),
            ("GIT_INDEX_FILE", index.to_str().unwrap()),
        ],
    );

    assert_plans(&fx, &run, &alias(1), "inherited GIT_DIR");
    assert!(
        other.join("o.txt").is_file(),
        "the other repository is not touched"
    );
}
