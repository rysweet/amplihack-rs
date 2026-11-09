//! #1969: the two new read-only helpers on their own.
//!
//! - `workflow_caller_branch.sh <repo_path>` (D2): exit 0 with `<X>` alone on
//!   stdout for a top-level run in a linked worktree on a non-default branch;
//!   exit 1 for the main clone, a detached HEAD, a default branch or a nested
//!   run (depth >= 2, with the D1 INFO line); exit 2 for a usage error or a
//!   branch name that `check-ref-format --branch` does not print back.
//! - `workflow_push_plan.sh <target>` (D5): one classification line against
//!   `refs/remotes/origin/<target>`, never against `@{u}`, with fully
//!   qualified refs so that a tag or local branch called `origin/<target>`
//!   cannot change the answer (SR-3); exit 2 on any failure, stdout empty.
//!
//! The default-branch table is run through the detection helper and the
//! planner together, so the two copies of the rule cannot drift (D2).

use std::fs;
use std::path::{Path, PathBuf};

use super::fixture::*;

/// The caller's branch in these tests: a slash, and no regex metacharacters.
pub const CALLER_BRANCH: &str = "claude/issue-1969-caller";

/// A linked worktree at `<main>/worktrees/<name>` on new branch `name`, cut
/// from local `main` with no upstream, with one commit of its own.
pub fn linked_on_new_branch(fx: &Fx, name: &str) -> PathBuf {
    let dir = fx.caller.join("worktrees").join(name);
    fx.git(
        &fx.caller,
        &[
            "worktree",
            "add",
            "-q",
            "--no-track",
            "-b",
            name,
            "--",
            dir.to_str().unwrap(),
            "main",
        ],
    );
    fx.commit_in(&dir, "caller.txt", "caller work\n", "caller work");
    dir
}

fn detect(fx: &Fx, dir: &Path, depth: Option<&str>) -> Run {
    let envs: Vec<(&str, &str)> = depth
        .map(|d| vec![("AMPLIHACK_SESSION_DEPTH", d)])
        .unwrap_or_default();
    fx.tool(DETECT, &fx.root, &[dir.to_str().unwrap()], &envs)
}

fn assert_continues(run: &Run, branch: &str, why: &str) {
    assert_eq!(run.code, Some(0), "{why}\n{}", run.describe());
    assert_eq!(
        run.stdout,
        format!("{branch}\n"),
        "{why}: the branch alone on stdout\n{}",
        run.describe()
    );
    let continuing = format!("continuing caller branch '{branch}'");
    let upstream = format!("origin/{branch}");
    assert!(
        run.stderr_line_with(&["INFO:", &continuing, "(#1969)", &upstream])
            .is_some(),
        "{why}: one INFO line names the branch, origin/<X> and #1969\n{}",
        run.describe()
    );
}

/// True when `stderr` has the continuation INFO line. The nested-run line
/// says "not continuing caller branch", which is not a continuation.
pub fn continues(stderr: &str) -> bool {
    stderr.lines().any(|l| {
        l.contains("continuing caller branch") && !l.contains("not continuing caller branch")
    })
}

fn assert_not_applicable(run: &Run, why: &str) {
    assert_eq!(run.code, Some(1), "{why}\n{}", run.describe());
    assert!(
        run.stdout.is_empty(),
        "{why}: nothing on stdout\n{}",
        run.describe()
    );
    assert!(
        !continues(&run.stderr),
        "{why}: no continuation INFO line\n{}",
        run.describe()
    );
}

/// D1: unset, empty, non-numeric, 0 and 1 are all top-level.
#[test]
fn detect_continues_top_level_linked_worktree_on_feature_branch() {
    let fx = Fx::new();
    let wt = linked_on_new_branch(&fx, CALLER_BRANCH);
    for depth in [
        None,
        Some(""),
        Some("0"),
        Some("1"),
        Some("abc"),
        Some("-3"),
        Some("2a"),
        Some(" 2"),
    ] {
        let run = detect(&fx, &wt, depth);
        assert_continues(&run, CALLER_BRANCH, &format!("depth {depth:?}"));
    }
}

/// D8: a subdirectory or a symbolic link to the linked worktree is detected
/// the same way. Tracked and untracked changes play no part here; the planner
/// checks them later (D3), and the helper never runs `git status`.
#[test]
fn detect_works_from_subdirectory_symlink_and_dirty_worktree() {
    let fx = Fx::new();
    let wt = linked_on_new_branch(&fx, CALLER_BRANCH);
    fx.commit_in(&wt, "sub/inner.txt", "inner\n", "inner");
    let link = fx.root.join("link-to-caller");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&wt, &link).expect("symlink");

    assert_continues(
        &detect(&fx, &wt.join("sub"), Some("1")),
        CALLER_BRANCH,
        "subdirectory",
    );
    #[cfg(unix)]
    assert_continues(&detect(&fx, &link, Some("1")), CALLER_BRANCH, "symlink");

    fx.write_in(&wt, "caller.txt", "tracked change\n");
    fx.write_in(&wt, "untracked.txt", "untracked\n");
    assert_continues(
        &detect(&fx, &wt, Some("1")),
        CALLER_BRANCH,
        "tracked and untracked changes",
    );
}

/// D1: a nested run (whole-number depth >= 2) keeps the #1134 behaviour and
/// says so, with the branch single-quoted in the hint (SR-14).
#[test]
fn detect_nested_run_keeps_1134_behaviour() {
    let fx = Fx::new();
    let wt = linked_on_new_branch(&fx, CALLER_BRANCH);
    for depth in ["2", "3", "10"] {
        let run = detect(&fx, &wt, Some(depth));
        let why = format!("depth {depth}");
        assert_not_applicable(&run, &why);
        let line = format!(
            "INFO: nested run (depth {depth}); not continuing caller branch '{CALLER_BRANCH}' (#1969, #1134). Pass -c existing_branch='{CALLER_BRANCH}' to continue it."
        );
        assert!(
            run.stderr.lines().any(|l| l == line),
            "{why}: expected the nested-run line\n  {line}\n{}",
            run.describe()
        );
    }
    // A leading zero is still a whole number; bash arithmetic would read 08
    // as invalid octal, so the comparison must not use $(( )).
    let run = detect(&fx, &wt, Some("08"));
    assert_not_applicable(&run, "depth 08");
    assert!(
        run.stderr_line_with(&["INFO: nested run", CALLER_BRANCH])
            .is_some(),
        "depth 08\n{}",
        run.describe()
    );
}

#[test]
fn detect_skips_main_clone_and_detached_head() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    fx.write_in(&fx.caller, "sub/x.txt", "x\n");
    assert_not_applicable(
        &detect(&fx, &fx.caller, Some("1")),
        "main clone on a feature branch",
    );
    assert_not_applicable(
        &detect(&fx, &fx.caller.join("sub"), Some("1")),
        "subdirectory of the main clone",
    );

    let detached = fx.root.join("detached");
    fx.git(
        &fx.caller,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            detached.to_str().unwrap(),
            "main",
        ],
    );
    assert_not_applicable(
        &detect(&fx, &detached, Some("1")),
        "linked worktree, detached HEAD",
    );
}

/// Exit 2 is an error: step 4 stops before creating anything (R7).
#[test]
fn detect_errors_on_bad_name_and_usage() {
    let fx = Fx::new();
    // A branch whose name `check-ref-format --branch` rejects.
    let odd = fx.root.join("odd");
    fx.git(
        &fx.caller,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            odd.to_str().unwrap(),
            "main",
        ],
    );
    fx.git(&fx.caller, &["update-ref", "refs/heads/-odd", "main"]);
    fx.git(&odd, &["symbolic-ref", "HEAD", "refs/heads/-odd"]);
    let run = detect(&fx, &odd, Some("1"));
    assert_eq!(run.code, Some(2), "invalid name\n{}", run.describe());
    assert!(run.stdout.is_empty(), "invalid name\n{}", run.describe());

    for args in [vec![], vec![".", "."]] {
        let run = fx.tool(DETECT, &fx.caller, &args, &[]);
        assert_eq!(run.code, Some(2), "usage {args:?}\n{}", run.describe());
        assert!(run.stdout.is_empty(), "usage {args:?}\n{}", run.describe());
    }

    let not_repo = fx.root.join("home");
    let missing = fx.root.join("does-not-exist");
    for dir in [&not_repo, &missing] {
        let run = fx.tool(
            DETECT,
            &fx.root,
            &[dir.to_str().unwrap()],
            &[
                ("GIT_CEILING_DIRECTORIES", fx.root.to_str().unwrap()),
                ("AMPLIHACK_SESSION_DEPTH", "1"),
            ],
        );
        assert_ne!(
            run.code,
            Some(0),
            "{}: never 'continue'\n{}",
            dir.display(),
            run.describe()
        );
        assert!(
            run.stdout.is_empty(),
            "{}\n{}",
            dir.display(),
            run.describe()
        );
    }
}

/// S7: an inherited GIT_DIR, GIT_WORK_TREE or GIT_INDEX_FILE cannot point the
/// helper at another repository.
#[test]
fn detect_ignores_inherited_git_dir() {
    let fx = Fx::new();
    let wt = linked_on_new_branch(&fx, CALLER_BRANCH);
    let other = fx.other_repo("other", "other-branch");
    let git_dir = other.join(".git");
    let index = git_dir.join("index");
    let run = fx.tool(
        DETECT,
        &wt,
        &[wt.to_str().unwrap()],
        &[
            ("GIT_DIR", git_dir.to_str().unwrap()),
            ("GIT_WORK_TREE", other.to_str().unwrap()),
            ("GIT_INDEX_FILE", index.to_str().unwrap()),
            ("AMPLIHACK_SESSION_DEPTH", "1"),
        ],
    );
    assert_continues(&run, CALLER_BRANCH, "inherited GIT_DIR");
}

/// V2: `AMPLIHACK_SESSION_DEPTH` is matched as text, never evaluated. Under
/// `[[ -ge ]]`, `$(( ))` or `let`, `a[$(touch PWNED)]` would run its command
/// substitution and `1+1` or `2#10` would count as depth 2; here every one of
/// them is "not a whole number", so the run is top-level.
#[test]
fn detect_never_evaluates_session_depth() {
    let fx = Fx::new();
    let wt = linked_on_new_branch(&fx, CALLER_BRANCH);
    for depth in [
        "a[$(touch PWNED)]",
        "x[`touch PWNED`]",
        "$(touch PWNED)",
        "1+1",
        "2#10",
        "2*1",
        "$((2))",
        "0x2",
    ] {
        let why = format!("depth {depth:?}");
        let run = detect(&fx, &wt, Some(depth));
        fx.assert_nothing_executed(&run, &why);
        assert_continues(&run, CALLER_BRANCH, &why);
        assert!(
            !run.stderr.contains("nested run"),
            "{why}: never read as a nested depth\n{}",
            run.describe()
        );
    }
}

/// V1, V6: a branch name that `check-ref-format --branch` accepts but that
/// holds a single quote, `$(..)` and backticks is printed back verbatim and
/// never run. The nested-run hint quotes it so that pasting
/// `existing_branch=<word>` into a shell gives back exactly the name.
#[test]
fn detect_treats_hostile_branch_name_as_data() {
    let fx = Fx::new();
    let name = format!("claude/{HOSTILE}");
    let check = fx.git(&fx.caller, &["check-ref-format", "--branch", &name]);
    assert_eq!(check.trim_end(), name, "fixture: git accepts the name");
    let wt = linked_on_new_branch(&fx, &name);

    let run = detect(&fx, &wt, Some("1"));
    fx.assert_nothing_executed(&run, "top-level, hostile name");
    assert_continues(&run, &name, "top-level, hostile name");

    let run = detect(&fx, &wt, Some("2"));
    fx.assert_nothing_executed(&run, "nested, hostile name");
    assert_not_applicable(&run, "nested, hostile name");
    let line = run
        .stderr_line_with(&["INFO: nested run (depth 2)", "-c existing_branch="])
        .unwrap_or_else(|| panic!("nested-run hint\n{}", run.describe()));
    let word = between(line, "-c existing_branch=", " to continue it.")
        .unwrap_or_else(|| panic!("hint shape: {line}"));
    assert!(
        word.starts_with('\'') && word.ends_with('\''),
        "the hint keeps the 'X' form: {word}"
    );
    assert_eq!(
        fx.shell_value(word),
        name,
        "pasting the hint gives back the branch name: {word}"
    );
    fx.assert_nothing_executed(&run, "pasting the nested-run hint");
}

/// D2: the detection helper and the planner carry the same default-branch
/// rule. The helper exits 1 for a linked worktree on `name` exactly when the
/// planner exits 3 for `name`, and both match the table.
#[test]
fn detect_and_planner_agree_on_default_branches() {
    // (branch, target of origin/HEAD, whether it is a default branch)
    let cases = [
        ("main", Some("main"), true),
        ("master", Some("main"), true),
        ("trunk", Some("trunk"), true),
        ("develop", Some("main"), false),
        ("develop", None, true),
        (CALLER_BRANCH, Some("main"), false),
    ];
    for (name, origin_head, default) in cases {
        let fx = Fx::new();
        // Free `main` so a linked worktree can hold it.
        fx.git(&fx.caller, &["switch", "-q", "--detach"]);
        match origin_head {
            Some("main") => {}
            Some(target) => {
                fx.git(
                    &fx.caller,
                    &["push", "-q", "origin", &format!("main:{target}")],
                );
                fx.git(&fx.caller, &["remote", "set-head", "origin", target]);
            }
            None => {
                fx.git(&fx.caller, &["remote", "set-head", "origin", "-d"]);
            }
        }
        let why = format!("`{name}`, origin/HEAD {origin_head:?}");
        let dir = fx.root.join("linked").join(name.replace('/', "_"));
        let mut args = vec!["worktree", "add", "-q"];
        if name != "main" {
            args.extend(["--no-track", "-b", name]);
        }
        let dir_s = dir.to_str().unwrap().to_owned();
        args.extend(["--", dir_s.as_str(), "main"]);
        fx.git(&fx.caller, &args);

        let detected = detect(&fx, &dir, Some("1"));
        let planned = fx.tool(
            PLANNER,
            &fx.root,
            &[dir.to_str().unwrap(), fx.caller.to_str().unwrap(), name],
            &[],
        );

        let detect_default = detected.code == Some(1);
        let planner_default = planned.code == Some(3);
        assert!(
            matches!(detected.code, Some(0) | Some(1)),
            "{why}: detection must decide\n{}",
            detected.describe()
        );
        assert_eq!(
            detect_default,
            planner_default,
            "{why}: detection and planner disagree\ndetect:\n{}\nplanner:\n{}",
            detected.describe(),
            planned.describe()
        );
        assert_eq!(
            detect_default,
            default,
            "{why}: the default-branch rule is wrong\n{}",
            detected.describe()
        );
    }
}

// ---------------------------------------------------------------------------
// workflow_push_plan.sh
// ---------------------------------------------------------------------------

fn plan_in(fx: &Fx, dir: &Path, target: &str) -> Run {
    fx.tool(PUSH_PLAN, dir, &[target], &[])
}

fn assert_plan(fx: &Fx, dir: &Path, target: &str, expected: &str, why: &str) {
    let run = plan_in(fx, dir, target);
    assert_eq!(run.code, Some(0), "{why}\n{}", run.describe());
    assert_eq!(
        run.stdout,
        format!("{expected}\n"),
        "{why}: one classification line\n{}",
        run.describe()
    );
}

fn rev(fx: &Fx, dir: &Path, spec: &str) -> String {
    fx.git(dir, &["rev-parse", spec]).trim().to_owned()
}

/// A commit on top of `parent` with the same tree, made without touching any
/// ref or the working tree.
fn sibling_commit(fx: &Fx, dir: &Path, parent: &str, message: &str) -> String {
    let tree = rev(fx, dir, &format!("{parent}^{{tree}}"));
    fx.git(dir, &["commit-tree", &tree, "-p", parent, "-m", message])
        .trim()
        .to_owned()
}

/// The four classifications, each against refs/remotes/origin/<target>.
#[test]
fn push_plan_classifies_against_origin_target() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let c = fx.caller.clone();

    assert_plan(
        &fx,
        &c,
        BRANCH,
        "already-pushed ahead=0 behind=0",
        "HEAD equals origin/<T>",
    );

    fx.commit_in(&c, "local.txt", "local\n", "local work");
    assert_plan(
        &fx,
        &c,
        BRANCH,
        "fast-forward ahead=1 behind=0",
        "HEAD strictly ahead",
    );

    // A stale @{u} (origin/main, two commits behind HEAD) changes nothing.
    fx.git(&c, &["branch", "-q", "--set-upstream-to=origin/main"]);
    assert_plan(
        &fx,
        &c,
        BRANCH,
        "fast-forward ahead=1 behind=0",
        "stale @{u} is never read",
    );

    // origin/<T> gains a commit HEAD lacks: diverged.
    let base = rev(&fx, &c, &format!("refs/remotes/origin/{BRANCH}"));
    let theirs = sibling_commit(&fx, &c, &base, "upstream work");
    fx.git(
        &c,
        &[
            "update-ref",
            &format!("refs/remotes/origin/{BRANCH}"),
            &theirs,
        ],
    );
    assert_plan(
        &fx,
        &c,
        BRANCH,
        &format!("diverged ahead=1 behind=1 merge_base={base} upstream=origin/{BRANCH}"),
        "both sides have commits",
    );

    // HEAD back at the old origin tip: strictly behind. ahead=0 wins.
    fx.git(&c, &["reset", "-q", "--hard", &base]);
    assert_plan(
        &fx,
        &c,
        BRANCH,
        "already-pushed ahead=0 behind=1",
        "strictly behind is already-pushed (ahead=0 rule first)",
    );
}

/// A missing refs/remotes/origin/<T> is a first push; the count is commits
/// on no origin branch. A tag or local branch called origin/<T> must not be
/// taken for the remote-tracking ref (SR-3).
#[test]
fn push_plan_first_push_and_shadowing_refs() {
    let fx = Fx::new();
    let target = "feat/issue-1969-plan";
    let wt = linked_on_new_branch(&fx, target);
    fx.commit_in(&wt, "second.txt", "second\n", "second");

    assert_plan(&fx, &wt, target, "first-push ahead=2", "not on origin");

    // A wrong upstream, and a tag and a local branch named origin/<T> at HEAD.
    fx.git(&wt, &["branch", "-q", "--set-upstream-to=origin/main"]);
    let shadow = format!("origin/{target}");
    fx.git(&wt, &["tag", &shadow, "HEAD"]);
    fx.git(&wt, &["branch", "-q", &shadow, "HEAD"]);
    assert_plan(
        &fx,
        &wt,
        target,
        "first-push ahead=2",
        "a tag or branch named origin/<T> is not the remote-tracking ref",
    );

    // Publish only the first commit: now a fast-forward of one, whatever the
    // shadowing refs at HEAD say.
    fx.git(
        &wt,
        &[
            "push",
            "-q",
            "origin",
            &format!("HEAD~1:refs/heads/{target}"),
        ],
    );
    assert_plan(
        &fx,
        &wt,
        target,
        "fast-forward ahead=1 behind=0",
        "origin/<T> one behind, with shadowing refs at HEAD",
    );
}

/// Exit 2 with nothing on stdout: usage errors, targets that do not survive
/// `check-ref-format --branch` unchanged, and counts that fail.
#[test]
fn push_plan_fails_closed() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let c = fx.caller.clone();

    for target in ["", "@{-1}", "-bad", "a..b", "feat/x.lock", "HEAD~1"] {
        let run = plan_in(&fx, &c, target);
        assert_eq!(run.code, Some(2), "target {target:?}\n{}", run.describe());
        assert!(
            run.stdout.is_empty(),
            "target {target:?}\n{}",
            run.describe()
        );
    }
    for args in [vec![], vec![BRANCH, BRANCH]] {
        let run = fx.tool(PUSH_PLAN, &c, &args, &[]);
        assert_eq!(run.code, Some(2), "usage {args:?}\n{}", run.describe());
        assert!(run.stdout.is_empty(), "usage {args:?}\n{}", run.describe());
    }

    // An unborn HEAD makes every count fail; that is an error, never zero.
    fx.git(&c, &["switch", "-q", "--orphan", "feat/issue-1969-unborn"]);
    for target in [BRANCH, "feat/issue-1969-unborn"] {
        let run = plan_in(&fx, &c, target);
        assert_eq!(
            run.code,
            Some(2),
            "unborn HEAD, target {target}\n{}",
            run.describe()
        );
        assert!(
            run.stdout.is_empty(),
            "unborn HEAD, target {target}\n{}",
            run.describe()
        );
    }
}

/// The plan only reads: refs, configuration and the index are unchanged.
#[test]
fn push_plan_only_reads_and_ignores_inherited_git_dir() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let c = fx.caller.clone();
    fx.commit_in(&c, "local.txt", "local\n", "local work");
    let state = |fx: &Fx| {
        (
            fx.git(&c, &["for-each-ref", "--format=%(refname) %(objectname)"]),
            fx.git(&c, &["config", "--local", "--list"]),
            fs::read(c.join(".git/index")).unwrap_or_default(),
        )
    };
    let before = state(&fx);

    let other = fx.other_repo("other", "other-branch");
    let git_dir = other.join(".git");
    let index = git_dir.join("index");
    let run = fx.tool(
        PUSH_PLAN,
        &c,
        &[BRANCH],
        &[
            ("GIT_DIR", git_dir.to_str().unwrap()),
            ("GIT_WORK_TREE", other.to_str().unwrap()),
            ("GIT_INDEX_FILE", index.to_str().unwrap()),
        ],
    );
    assert_eq!(run.code, Some(0), "{}", run.describe());
    assert_eq!(
        run.stdout,
        "fast-forward ahead=1 behind=0\n",
        "the caller's repository, not the inherited one\n{}",
        run.describe()
    );
    assert!(state(&fx) == before, "the plan must change nothing");
}
