//! #1969: step 4 continues the caller's branch when a top-level run's
//! `repo_path` is a linked worktree on its own feature branch (R6), refuses up
//! front when that branch cannot be continued (R7), and keeps today's
//! behaviour everywhere else (R8).
//!
//! The caller's worktree sits at `<main>/worktrees/<X>`, as in the incident.
//! Every case sets `AMPLIHACK_SESSION_DEPTH` explicitly: `1` is what a
//! top-level run's steps see, `2` or more is a nested run (D1).

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::fixture::*;
use super::issue_1969_helpers::{CALLER_BRANCH as X, continues, linked_on_new_branch};
use super::issue_1969_publish::step15;

const TASK: &str = "implement the delegation receiver";
const DERIVED_PREFIX: &str = "feat/issue-1969-";

/// Run workflow-worktree step 4 with `REPO_PATH=repo_path`, no branch named,
/// and `AMPLIHACK_SESSION_DEPTH=depth` when one is given. `extra` overrides any
/// variable; `home` replaces `AMPLIHACK_HOME`.
pub fn step4_from(
    fx: &Fx,
    repo_path: &Path,
    depth: Option<&str>,
    extra: &[(&str, &str)],
    home: Option<&Path>,
) -> Run {
    let mut command = fx.bash(repo_path);
    command
        .arg("-c")
        .arg(step_text(
            "workflow-worktree",
            "step-04-setup-worktree",
            "command",
        ))
        .env("REPO_PATH", repo_path)
        .env("EXISTING_BRANCH", "")
        .env("PR_NUMBER", "")
        .env("BRANCH_PREFIX", "feat")
        .env("ISSUE_NUMBER", "1969")
        .env("TASK_DESCRIPTION", TASK);
    if let Some(depth) = depth {
        command.env("AMPLIHACK_SESSION_DEPTH", depth);
    }
    if let Some(home) = home {
        command.env("AMPLIHACK_HOME", home);
    }
    for (key, value) in extra {
        command.env(key, value);
    }
    run(&mut command)
}

/// The caller: a linked worktree at `<main>/worktrees/<X>` on `<X>`, cut from
/// main with one commit, pushed with `-u` when `pushed`.
fn caller_worktree(fx: &Fx, pushed: bool) -> PathBuf {
    let wt = linked_on_new_branch(fx, X);
    if pushed {
        fx.git(&wt, &["push", "-q", "-u", "origin", X]);
    }
    wt
}

/// Branches, registered worktrees and origin refs: what "nothing created"
/// and "no other new branch or worktree" are measured against.
#[derive(Debug, PartialEq, Eq, Clone)]
struct Snapshot {
    branches: Vec<String>,
    worktrees: Vec<String>,
    origin: Vec<String>,
}

fn snapshot(fx: &Fx) -> Snapshot {
    let mut branches: Vec<String> = fx
        .git(
            &fx.caller,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads/",
            ],
        )
        .lines()
        .map(str::to_owned)
        .collect();
    branches.sort();
    let mut worktrees: Vec<String> = fx
        .git(&fx.caller, &["worktree", "list", "--porcelain"])
        .lines()
        .filter(|l| l.starts_with("worktree "))
        .map(str::to_owned)
        .collect();
    worktrees.sort();
    let mut origin = fx.origin_refs();
    origin.sort();
    Snapshot {
        branches,
        worktrees,
        origin,
    }
}

/// HEAD, HEAD's commit and tracked status of the caller's worktree.
fn caller_view(fx: &Fx, wt: &Path) -> (String, String, String) {
    (
        fx.git(wt, &["symbolic-ref", "-q", "HEAD"]),
        fx.head(wt),
        fx.git(
            wt,
            &[
                "--no-optional-locks",
                "status",
                "--porcelain",
                "--untracked-files=no",
            ],
        ),
    )
}

fn continuation_line(run: &Run) -> Option<&str> {
    let continuing = format!("continuing caller branch '{X}'");
    let upstream = format!("origin/{X}");
    run.stderr_line_with(&["INFO:", &continuing, "(#1969)", &upstream])
}

fn assert_alias_continuation(fx: &Fx, run: &Run, why: &str) -> Value {
    assert_eq!(run.code, Some(0), "{why}\n{}", run.describe());
    let json = run.json();
    let alias = format!("{X}--wt-1");
    assert_eq!(json["branch_name"], X, "{why}\n{}", run.describe());
    assert_eq!(
        json["local_branch"],
        alias.as_str(),
        "{why}\n{}",
        run.describe()
    );
    assert_eq!(
        json["worktree_path"],
        fx.caller.join("worktrees").join(&alias).to_str().unwrap(),
        "{why}: the alias worktree is <main>/worktrees/<X>--wt-1\n{}",
        run.describe()
    );
    assert_eq!(json["created"], false, "{why}\n{}", run.describe());
    assert_eq!(
        json["local_branch_reason"],
        "branch_name is checked out in the caller repository (issue #1762)",
        "{why}: the JSON schema is the alias path's (R6)\n{}",
        run.describe()
    );
    assert!(
        continuation_line(run).is_some(),
        "{why}: the INFO line names <X>, origin/<X> and #1969\n{}",
        run.describe()
    );
    assert_eq!(
        fx.config_get(&format!("branch.{alias}.merge")),
        format!("refs/heads/{X}"),
        "{why}: the alias tracks origin/<X>"
    );
    json
}

/// R11(c): a clean, pushed caller branch is continued through the #1762
/// alias; the run's commits reach origin/<X>; nothing else is created and
/// the caller's worktree is untouched. An untracked file does not stop it.
#[test]
fn continues_clean_pushed_caller_branch() {
    let fx = Fx::new();
    let wt = caller_worktree(&fx, true);
    fx.write_in(&wt, "notes-untracked.txt", "scratch\n");
    let before = snapshot(&fx);
    let view = caller_view(&fx, &wt);

    let run = step4_from(&fx, &wt, Some("1"), &[], None);

    let setup = assert_alias_continuation(&fx, &run, "clean, pushed caller");
    let alias = format!("{X}--wt-1");
    let alias_path = fx.caller.join("worktrees").join(&alias);

    let after = snapshot(&fx);
    let new_branches: Vec<&String> = after
        .branches
        .iter()
        .filter(|b| !before.branches.contains(b))
        .collect();
    assert_eq!(
        new_branches.len(),
        1,
        "only the alias is a new branch (R11 c): {new_branches:?}"
    );
    assert!(
        new_branches[0].starts_with(&format!("refs/heads/{alias} ")),
        "the new branch is the alias: {new_branches:?}"
    );
    let new_worktrees: Vec<&String> = after
        .worktrees
        .iter()
        .filter(|w| !before.worktrees.contains(w))
        .collect();
    assert_eq!(
        new_worktrees,
        vec![&format!("worktree {}", alias_path.display())],
        "only the alias worktree is new"
    );
    assert_eq!(after.origin, before.origin, "step 4 pushes nothing here");
    assert!(
        !run.stderr.contains("Creating new branch"),
        "no derived branch\n{}",
        run.describe()
    );
    assert_eq!(
        caller_view(&fx, &wt),
        view,
        "the caller's worktree is untouched"
    );

    // The run's commits reach origin/<X>.
    fx.write_in(&alias_path, "delegation.txt", "receiver model\n");
    let publish = step15(&fx, &alias_path, &setup, None);
    assert_eq!(publish.code, Some(0), "{}", publish.describe());
    assert_eq!(publish.json()["reason"], "pushed", "{}", publish.describe());
    assert_eq!(
        fx.origin_sha(X).as_deref(),
        Some(fx.head(&alias_path).as_str()),
        "origin/<X> holds the run's commit"
    );
    fx.assert_no_alias_on_origin("caller-branch continuation");
}

/// D8: a caller behind origin/<X> is accepted and the alias starts at the
/// fetched origin/<X>; a `repo_path` in a subdirectory of the linked
/// worktree is detected the same way, and Gate B compares the top level.
#[test]
fn continues_from_subdirectory_when_caller_is_behind() {
    let fx = Fx::new();
    let wt = caller_worktree(&fx, false);
    fx.commit_in(&wt, "sub/inner.txt", "inner\n", "inner");
    fx.git(&wt, &["push", "-q", "-u", "origin", X]);

    // Another clone moves origin/<X> on; the caller's tracking ref is stale.
    let other = fx.root.join("other-clone");
    fx.git(
        &fx.root,
        &["clone", "-q", ORIGIN_URL, other.to_str().unwrap()],
    );
    fx.git(&other, &["config", "user.email", "other@example.com"]);
    fx.git(&other, &["config", "user.name", "Other"]);
    fx.git(&other, &["config", "commit.gpgsign", "false"]);
    fx.git(&other, &["switch", "-q", X]);
    fx.commit_in(&other, "upstream.txt", "upstream\n", "upstream work");
    fx.git(&other, &["push", "-q", "origin", X]);
    let origin_tip = fx.head(&other);
    let caller_head = fx.head(&wt);

    let run = step4_from(&fx, &wt.join("sub"), Some("1"), &[], None);

    let setup = assert_alias_continuation(&fx, &run, "subdirectory, caller behind");
    let alias_path = PathBuf::from(setup["worktree_path"].as_str().unwrap());
    assert_eq!(
        fx.head(&alias_path),
        origin_tip,
        "the alias starts at the fetched origin/<X>"
    );
    assert_eq!(fx.head(&wt), caller_head, "the caller is not moved");
}

/// R7, R11(d): an unpushed, ahead or tracked-dirty caller stops step 4 with
/// exit 1 before anything is created, with the planner's reason, the branch
/// and the hint, which names the main clone and never offers
/// `existing_branch=<X>` (that re-run would reuse the caller's worktree as
/// it stands; see `following_the_stop_hint_never_touches_a_dirty_caller`).
#[test]
fn refuses_unusable_caller_branch_up_front() {
    type Prepare = fn(&Fx, &Path);
    let cases: [(&str, bool, Prepare, String); 3] = [
        (
            "unpushed",
            false,
            |_, _| {},
            format!("origin/{X} does not exist"),
        ),
        (
            "ahead",
            true,
            |fx, wt| fx.commit_in(wt, "ahead.txt", "ahead\n", "local only"),
            format!("commit(s) that are not on origin/{X}"),
        ),
        (
            "tracked-dirty",
            true,
            |fx, wt| fx.write_in(wt, "caller.txt", "uncommitted change\n"),
            "uncommitted changes to tracked files".to_owned(),
        ),
    ];
    for (why, pushed, prepare, reason) in cases {
        let fx = Fx::new();
        let wt = caller_worktree(&fx, pushed);
        prepare(&fx, &wt);
        let before = snapshot(&fx);
        let view = caller_view(&fx, &wt);

        let run = step4_from(&fx, &wt, Some("1"), &[], None);

        assert_eq!(run.code, Some(1), "{why}\n{}", run.describe());
        assert!(
            !run.stdout.contains('{'),
            "{why}: no JSON on stdout\n{}",
            run.describe()
        );
        assert_eq!(snapshot(&fx), before, "{why}: no branch, worktree or push");
        assert!(
            !fx.caller.join("worktrees/feat").exists(),
            "{why}: never falls back to a derived branch"
        );
        assert!(
            !fx.caller.join(format!("worktrees/{X}--wt-1")).exists(),
            "{why}: no alias directory"
        );
        assert_eq!(caller_view(&fx, &wt), view, "{why}: caller untouched");
        assert!(
            run.stderr_line_with(&["ERROR:", X, &reason]).is_some(),
            "{why}: the planner's own reason, naming <X>\n{}",
            run.describe()
        );
        assert_eq!(
            stop_hint_path(&run, X),
            fx.caller.to_str().unwrap(),
            "{why}: the stop hint names <X> and the main clone\n{}",
            run.describe()
        );
    }
}

/// R7, D2: a detection helper that fails (exit 2 or more) stops step 4
/// before anything is created; so does one whose answer is not a valid
/// branch name.
#[test]
fn failing_detection_helper_stops_before_anything() {
    let fx = Fx::new();
    let wt = caller_worktree(&fx, true);
    let home = fx.bundle_with(
        DETECT,
        "#!/usr/bin/env bash\necho 'stub: detection failed' >&2\nexit 3\n",
    );
    let before = snapshot(&fx);

    let run = step4_from(&fx, &wt, Some("1"), &[], Some(&home));

    assert_eq!(run.code, Some(1), "{}", run.describe());
    assert!(
        run.stderr_line_with(&[
            "ERROR:",
            "caller-branch detection failed",
            "workflow_caller_branch.sh exit 3",
            "(#1969)"
        ])
        .is_some(),
        "{}",
        run.describe()
    );
    assert!(!run.stdout.contains('{'), "{}", run.describe());
    assert_eq!(snapshot(&fx), before, "nothing created");
    assert!(!fx.caller.join("worktrees/feat").exists());

    let home = fx.bundle_with(DETECT, "#!/usr/bin/env bash\necho '-oops'\nexit 0\n");
    let run = step4_from(&fx, &wt, Some("1"), &[], Some(&home));
    assert_ne!(
        run.code,
        Some(0),
        "an invalid name is refused\n{}",
        run.describe()
    );
    assert!(!run.stdout.contains('{'), "{}", run.describe());
    assert_eq!(snapshot(&fx), before, "nothing created");
    assert!(!fx.caller.join("worktrees/feat").exists());
}

/// R9, D2: a missing detection helper warns, names the workaround and keeps
/// today's behaviour; it never switches silently.
#[test]
fn missing_detection_helper_warns_and_starts_fresh_branch() {
    let fx = Fx::new();
    let wt = caller_worktree(&fx, true);
    let home = fx.bundle_without(DETECT);

    let run = step4_from(&fx, &wt, Some("1"), &[], Some(&home));

    assert_eq!(run.code, Some(0), "{}", run.describe());
    let warning = "WARNING: workflow_caller_branch.sh not found; caller-branch continuation (#1969) disabled. Pass -c existing_branch=<branch> to continue a caller branch.";
    assert!(
        run.stderr.lines().any(|l| l.contains(warning)),
        "expected `{warning}`\n{}",
        run.describe()
    );
    let json = run.json();
    assert!(
        json["branch_name"]
            .as_str()
            .is_some_and(|b| b.starts_with(DERIVED_PREFIX)),
        "{}",
        run.describe()
    );
    assert!(json.get("local_branch").is_none(), "{}", run.describe());
}

/// R8, R11(e): main clone on a feature branch, a linked worktree on the
/// default branch, a detached HEAD and nested runs keep today's fresh branch.
/// A nested run says why it did not continue; an unpushed caller is not
/// refused at depth 2 (the #1134 Part B shape).
#[test]
fn unchanged_cases_start_a_fresh_branch() {
    type Setup = fn(&Fx) -> PathBuf;
    let cases: [(&str, &str, Setup); 5] = [
        ("main clone on a feature branch", "1", |fx| {
            fx.git(&fx.caller, &["switch", "-q", "-c", X]);
            fx.commit_in(&fx.caller, "caller.txt", "caller\n", "caller work");
            fx.git(&fx.caller, &["push", "-q", "-u", "origin", X]);
            fx.caller.clone()
        }),
        ("linked worktree on the default branch", "1", |fx| {
            fx.git(&fx.caller, &["switch", "-q", "-c", "parking"]);
            let dir = fx.root.join("linked-main");
            fx.git(
                &fx.caller,
                &["worktree", "add", "-q", "--", dir.to_str().unwrap(), "main"],
            );
            dir
        }),
        ("linked worktree, detached HEAD", "1", |fx| {
            let dir = fx.root.join("linked-detached");
            fx.git(
                &fx.caller,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "--detach",
                    dir.to_str().unwrap(),
                    "main",
                ],
            );
            dir
        }),
        ("nested run, unpushed caller", "2", |fx| {
            caller_worktree(fx, false)
        }),
        ("nested run, pushed caller", "3", |fx| {
            caller_worktree(fx, true)
        }),
    ];
    for (why, depth, setup) in cases {
        let fx = Fx::new();
        let repo_path = setup(&fx);

        let run = step4_from(&fx, &repo_path, Some(depth), &[], None);

        assert_eq!(run.code, Some(0), "{why}\n{}", run.describe());
        let json = run.json();
        let branch = json["branch_name"].as_str().unwrap_or_default().to_owned();
        assert!(
            branch.starts_with(DERIVED_PREFIX),
            "{why}: a fresh, derived branch\n{}",
            run.describe()
        );
        assert!(
            json.get("local_branch").is_none(),
            "{why}\n{}",
            run.describe()
        );
        assert_eq!(
            json["worktree_path"],
            fx.caller.join("worktrees").join(&branch).to_str().unwrap(),
            "{why}: anchored at the main clone (#1134)\n{}",
            run.describe()
        );
        assert!(
            continuation_line(&run).is_none() && !continues(&run.stderr),
            "{why}: no continuation\n{}",
            run.describe()
        );
        assert_ne!(
            fx.config_get(&format!("branch.{branch}.merge")),
            "refs/heads/main",
            "{why}: the new branch never tracks origin/main"
        );
        if depth != "1" {
            let line = format!(
                "INFO: nested run (depth {depth}); not continuing caller branch '{X}' (#1969, #1134). Pass -c existing_branch='{X}' to continue it."
            );
            assert!(
                run.stderr.lines().any(|l| l.contains(&line)),
                "{why}: expected `{line}`\n{}",
                run.describe()
            );
        }
    }
}

/// R8, R11(f): `existing_branch`, `pr_number` and a `Branch:` line win;
/// detection does not run.
#[test]
fn explicit_branch_wins_over_detection() {
    const OTHER: &str = "feat/issue-1969-other";
    const NAMED: &str = "feat/issue-1969-named";
    let named_task = format!("Branch: {NAMED}\n\n{TASK}");
    /// (case, variables to set, the branch that must win)
    type Case<'a> = (&'a str, Vec<(&'a str, &'a str)>, &'a str);
    let cases: [Case; 3] = [
        ("existing_branch", vec![("EXISTING_BRANCH", OTHER)], OTHER),
        ("pr_number", vec![("PR_NUMBER", "7")], OTHER),
        (
            "Branch: line",
            vec![("TASK_DESCRIPTION", named_task.as_str())],
            NAMED,
        ),
    ];
    for (why, extra, expected) in cases {
        let fx = Fx::new();
        let wt = caller_worktree(&fx, true);
        fx.git(&fx.caller, &["branch", "-q", OTHER, "main"]);
        fx.git(&fx.caller, &["push", "-q", "origin", OTHER]);
        // The fake gh prints view.json for `pr view`; workflow_pr_branch.sh
        // asks for `-q .headRefName`, so the file holds just the name.
        std::fs::write(fx.root.join("view.json"), format!("{OTHER}\n")).expect("view.json");
        let view = caller_view(&fx, &wt);

        let run = step4_from(&fx, &wt, Some("1"), &extra, None);

        assert_eq!(run.code, Some(0), "{why}\n{}", run.describe());
        assert_eq!(
            run.json()["branch_name"],
            expected,
            "{why}: the named branch wins\n{}",
            run.describe()
        );
        assert!(
            !run.stderr.contains("continuing caller branch")
                && !run.stderr.contains("not continuing caller branch"),
            "{why}: detection does not run\n{}",
            run.describe()
        );
        assert_eq!(caller_view(&fx, &wt), view, "{why}: caller untouched");
    }
}

/// A `repo_path` that is not a repository exits 1 and creates nothing.
#[test]
fn repo_path_not_a_repository_creates_nothing() {
    let fx = Fx::new();
    let dir = fx.root.join("not-a-repo");
    std::fs::create_dir_all(&dir).expect("dir");
    let before = snapshot(&fx);

    let run = step4_from(
        &fx,
        &dir,
        Some("1"),
        &[("GIT_CEILING_DIRECTORIES", fx.root.to_str().unwrap())],
        None,
    );

    assert_eq!(run.code, Some(1), "{}", run.describe());
    assert!(!run.stdout.contains('{'), "{}", run.describe());
    assert_eq!(snapshot(&fx), before, "nothing created");
    assert!(
        std::fs::read_dir(&dir).expect("read dir").next().is_none(),
        "nothing written into repo_path"
    );
}

/// The stop hint's one copy-paste word, `repo_path=<word>`, from the
/// "continuing the caller's branch" line. The line must not offer
/// `existing_branch=<value>` for any stop reason: Gate B reuses a worktree
/// that is not the caller's in place, so that re-run would commit whatever
/// the caller's worktree holds.
fn stop_hint_path(run: &Run, branch: &str) -> String {
    let continuing = format!("continuing the caller's branch '{branch}'");
    let line = run
        .stderr_line_with(&["ERROR:", &continuing, "(#1969)", "-c repo_path="])
        .unwrap_or_else(|| panic!("the stop hint names the branch\n{}", run.describe()));
    assert!(
        !line.contains("existing_branch="),
        "the stop hint never offers an existing_branch= re-run: {line}"
    );
    assert!(
        line.contains("Do not name the branch with -c existing_branch"),
        "the stop hint warns against the existing_branch route: {line}"
    );
    between(line, "-c repo_path=", ". Do not name the branch")
        .unwrap_or_else(|| panic!("hint shape: {line}"))
        .to_owned()
}

/// V6, D6: the stop hint for a branch whose name holds a single quote,
/// `$(..)` and backticks is shell-inert: pasting `repo_path=<word>` gives
/// back the main clone, and nothing runs, in step 4 or in the paste.
#[test]
fn stop_hint_for_hostile_branch_name_is_shell_inert() {
    let fx = Fx::new();
    let name = format!("claude/{HOSTILE}");
    let wt = linked_on_new_branch(&fx, &name);
    let before = snapshot(&fx);

    let run = step4_from(&fx, &wt, Some("1"), &[], None);

    fx.assert_nothing_executed(&run, "step 4 refusing a hostile name");
    assert_eq!(run.code, Some(1), "unpushed: refused\n{}", run.describe());
    assert!(!run.stdout.contains('{'), "{}", run.describe());
    assert_eq!(snapshot(&fx), before, "nothing created");
    assert!(
        run.stderr_line_with(&["ERROR:", &name, &format!("origin/{name} does not exist")])
            .is_some(),
        "the planner's reason names the branch verbatim\n{}",
        run.describe()
    );

    let path_word = stop_hint_path(&run, &name);
    assert_eq!(
        fx.shell_value(&path_word),
        fx.caller.to_str().unwrap(),
        "pasted: {path_word}"
    );
    fx.assert_nothing_executed(&run, "pasting the stop hint");
}

/// V6: a plain path renders unchanged in the stop hint
/// (`repo_path=<main clone>`), as the docs quote it.
#[test]
fn stop_hint_for_plain_names_is_unchanged() {
    let fx = Fx::new();
    let wt = caller_worktree(&fx, false);
    let main = fx.caller.to_str().unwrap().to_owned();
    assert!(
        main.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)),
        "fixture: the main clone's path needs no quoting: {main}"
    );

    let run = step4_from(&fx, &wt, Some("1"), &[], None);

    assert_eq!(run.code, Some(1), "{}", run.describe());
    assert_eq!(stop_hint_path(&run, X), main, "{}", run.describe());
}

/// Crusty round 1: the stop hint after a dirty-tree stop is safe to follow
/// as printed. The caller's worktree has an uncommitted change to a tracked
/// file and an untracked file; step 4 refuses; re-running with the hint's
/// `repo_path=<main clone>` starts a new branch in a new worktree; step 15
/// there commits and pushes only the run's own file. The caller's worktree,
/// its branch and origin/<X> are untouched throughout.
#[test]
fn following_the_stop_hint_never_touches_a_dirty_caller() {
    let fx = Fx::new();
    let wt = caller_worktree(&fx, true);
    fx.write_in(&wt, "caller.txt", "uncommitted change\n");
    fx.write_in(&wt, "notes-untracked.txt", "scratch\n");
    let view = caller_view(&fx, &wt);
    let untracked = fx.git(&wt, &["status", "--porcelain", "--untracked-files=all"]);
    let origin_x = fx.origin_sha(X);

    let refused = step4_from(&fx, &wt, Some("1"), &[], None);
    assert_eq!(refused.code, Some(1), "{}", refused.describe());
    let hint_path = PathBuf::from(stop_hint_path(&refused, X));

    let rerun = step4_from(&fx, &hint_path, Some("1"), &[], None);
    assert_eq!(rerun.code, Some(0), "{}", rerun.describe());
    let setup = rerun.json();
    let branch = setup["branch_name"]
        .as_str()
        .expect("branch_name")
        .to_owned();
    assert!(
        branch.starts_with(DERIVED_PREFIX),
        "a new derived branch, not <X>: {branch}\n{}",
        rerun.describe()
    );
    assert!(setup.get("local_branch").is_none(), "{}", rerun.describe());
    let task = PathBuf::from(setup["worktree_path"].as_str().expect("worktree_path"));
    assert_ne!(
        task.canonicalize().unwrap(),
        wt.canonicalize().unwrap(),
        "the task worktree is never the caller's"
    );

    fx.write_in(&task, "delegation.txt", "receiver model\n");
    let publish = step15(&fx, &task, &setup, None);
    assert_eq!(publish.code, Some(0), "{}", publish.describe());
    assert_eq!(publish.json()["pushed"], "true", "{}", publish.describe());
    let committed = fx.git(&task, &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(
        committed.trim(),
        "delegation.txt",
        "only the run's file is committed"
    );

    assert_eq!(caller_view(&fx, &wt), view, "the caller is untouched");
    assert_eq!(
        fx.git(&wt, &["status", "--porcelain", "--untracked-files=all"]),
        untracked,
        "the caller's untracked file is still untracked"
    );
    assert_eq!(fx.origin_sha(X), origin_x, "origin/<X> does not move");
}

/// Why the hint does not offer `existing_branch=<X>`: from the main clone,
/// Gate B reuses the caller's worktree in place, because `existing_branch`
/// is how a kept worktree is continued with its changes (#1565). The step's
/// output names the caller's worktree, so step 15 would stage there.
#[test]
fn existing_branch_from_main_clone_reuses_the_caller_worktree_in_place() {
    let fx = Fx::new();
    let wt = caller_worktree(&fx, true);
    fx.write_in(&wt, "caller.txt", "uncommitted change\n");

    let run = step4_from(&fx, &fx.caller, Some("1"), &[("EXISTING_BRANCH", X)], None);

    assert_eq!(run.code, Some(0), "{}", run.describe());
    let setup = run.json();
    assert_eq!(setup["branch_name"], X, "{}", run.describe());
    assert_eq!(
        PathBuf::from(setup["worktree_path"].as_str().expect("worktree_path")),
        wt.canonicalize().unwrap(),
        "Gate B reuses the caller's worktree as it stands\n{}",
        run.describe()
    );
}

/// V1, V4: a clean, pushed caller branch with a hostile name is continued
/// through the #1762 alias like any other, and step 15 publishes the run's
/// commit to origin/<name>; no step evaluates the name.
#[test]
fn continues_hostile_caller_branch_as_data() {
    let fx = Fx::new();
    let name = format!("claude/{HOSTILE}");
    let wt = linked_on_new_branch(&fx, &name);
    fx.git(&wt, &["push", "-q", "-u", "origin", &name]);
    let main_before = fx.origin_sha("main");

    let run = step4_from(&fx, &wt, Some("1"), &[], None);

    fx.assert_nothing_executed(&run, "step 4 continuing a hostile name");
    assert_eq!(run.code, Some(0), "{}", run.describe());
    let setup = run.json();
    let alias = format!("{name}--wt-1");
    assert_eq!(setup["branch_name"], name.as_str(), "{}", run.describe());
    assert_eq!(setup["local_branch"], alias.as_str(), "{}", run.describe());
    assert_eq!(
        fx.config_get(&format!("branch.{alias}.merge")),
        format!("refs/heads/{name}"),
        "the alias tracks origin/<name>"
    );
    let task = PathBuf::from(setup["worktree_path"].as_str().expect("worktree_path"));
    assert!(task.is_dir(), "{}", run.describe());

    fx.write_in(&task, "delegation.txt", "receiver model\n");
    let push = step15(&fx, &task, &setup, None);

    fx.assert_nothing_executed(&push, "step 15 in a hostile alias");
    assert_eq!(push.code, Some(0), "{}", push.describe());
    assert_eq!(push.json()["pushed"], "true", "{}", push.describe());
    assert_eq!(
        fx.origin_sha(&name).as_deref(),
        Some(fx.head(&task).as_str()),
        "the run's commit reaches origin/<name>\n{}",
        push.describe()
    );
    assert_eq!(
        fx.origin_sha(&alias),
        None,
        "the alias never reaches origin"
    );
    assert_eq!(
        fx.origin_sha("main"),
        main_before,
        "origin/main does not move"
    );
}
