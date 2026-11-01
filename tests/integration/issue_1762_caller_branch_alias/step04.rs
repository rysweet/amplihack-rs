//! Step 4 on the alias path (#1762, D5, D6, D8): what it creates, what it
//! prints, when it reuses an alias and when it refuses to, and what it leaves
//! alone. The planner's stops are in `step04_stops.rs`.

use std::fs;
use std::path::Path;

use regex::Regex;

use super::fixture::*;

/// The R8 INFO line names the branch, the alias, origin/<B>, the caller's
/// physical path and #1762 (AC4).
fn assert_info_line(run: &Run, caller: &Path, alias: &str) {
    let caller = caller.to_str().expect("utf-8 path");
    let quoted = format!("'{BRANCH}'");
    let upstream = format!("origin/{BRANCH}");
    assert!(
        run.stderr_line_with(&["INFO:", &quoted, alias, &upstream, caller, "#1762"])
            .is_some(),
        "one stderr line must log the alias and why: {quoted}, {alias}, {upstream}, {caller}, #1762\n{}",
        run.describe()
    );
}

fn worktree_path(run: &Run) -> String {
    run.json()["worktree_path"]
        .as_str()
        .unwrap_or_else(|| panic!("worktree_path must be a string\n{}", run.describe()))
        .to_owned()
}

#[test]
fn alias_created_when_caller_on_pushed_clean_branch() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    // Untracked files are not checked (D4) and stay with the caller.
    fx.write_in(&fx.caller, "scratch-notes.txt", "untracked caller file\n");
    let caller_head = fx.head(&fx.caller);
    let before = fx.caller_state(&fx.caller);

    let run = fx.step4(BRANCH, &[]);

    assert_eq!(
        run.code,
        Some(0),
        "a clean, pushed caller on the target branch must get an alias worktree\n{}",
        run.describe()
    );
    let alias = alias(1);
    let expected = fx.caller.join("worktrees").join(&alias);
    let json = run.json();
    assert_eq!(
        json["branch_name"],
        BRANCH,
        "branch_name stays <B>\n{}",
        run.describe()
    );
    assert_eq!(json["local_branch"], alias.as_str(), "{}", run.describe());
    // recipe-runner-rs keeps a passing step's output but not its stderr, so the
    // reason goes in the output too, where the run log has it.
    assert_eq!(
        json["local_branch_reason"],
        "branch_name is checked out in the caller repository (issue #1762)",
        "the step output must give the reason for the alias\n{}",
        run.describe()
    );
    assert_eq!(
        json["created"], false,
        "created describes <B>, which existed"
    );
    assert_eq!(
        worktree_path(&run),
        expected.to_str().unwrap(),
        "the alias worktree is <main>/worktrees/<alias>, as a physical path"
    );
    let line = run
        .stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .expect("JSON line");
    let last_key = Regex::new(&format!(
        r#""local_branch"\s*:\s*"{}"\s*\}}\s*$"#,
        regex::escape(&alias)
    ))
    .unwrap();
    assert!(
        last_key.is_match(line),
        "local_branch must be the last JSON key: {line}"
    );
    assert!(
        Regex::new(r"--wt-[0-9]+$").unwrap().is_match(&alias),
        "AC1 alias pattern"
    );

    assert_eq!(
        fx.git(&expected, &["symbolic-ref", "--short", "HEAD"])
            .trim(),
        alias,
        "the task worktree is on the alias"
    );
    assert_eq!(
        fx.head(&expected),
        caller_head,
        "the alias starts at the same commit"
    );
    assert_eq!(fx.config_get(&format!("branch.{alias}.remote")), "origin");
    assert_eq!(
        fx.config_get(&format!("branch.{alias}.merge")),
        format!("refs/heads/{BRANCH}"),
        "the alias's upstream is the original remote branch"
    );
    assert_info_line(&run, &fx.caller, &alias);

    assert_eq!(
        fx.caller_state(&fx.caller),
        before,
        "the caller's HEAD, branches, index, tracked files, stash and branch.<B>.* must not change (R3)"
    );
    assert_eq!(
        fs::read_to_string(fx.caller.join("scratch-notes.txt")).unwrap(),
        "untracked caller file\n"
    );
    assert!(
        !expected.join("scratch-notes.txt").exists(),
        "the caller's untracked file must not appear in the task worktree"
    );
    assert_eq!(
        fx.origin_sha(BRANCH).as_deref(),
        Some(caller_head.as_str()),
        "step 4 must not move origin/<B>"
    );
    fx.assert_no_alias_on_origin("step 4 skips its best-effort push on the alias path");
}

/// Commit on the caller's branch and push it to origin/<B>: the planner's
/// "push them, then re-run" remedy. Returns the new commit.
fn push_new_commit_from_caller(fx: &Fx) -> String {
    fx.commit_in(
        &fx.caller,
        "pushed-later.txt",
        "later\n",
        "pushed between runs",
    );
    fx.git(&fx.caller, &["push", "-q", "origin", BRANCH]);
    fx.head(&fx.caller)
}

#[test]
fn rerun_reuses_alias_worktree() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let first = fx.step4(BRANCH, &[]);
    assert_eq!(first.code, Some(0), "first run\n{}", first.describe());
    let path = worktree_path(&first);
    let first_head = fx.head(Path::new(&path));

    let second = fx.step4(BRANCH, &[]);

    assert_eq!(second.code, Some(0), "rerun\n{}", second.describe());
    assert_eq!(second.json()["local_branch"], alias(1).as_str());
    assert_eq!(
        worktree_path(&second),
        path,
        "the rerun reuses the same worktree"
    );
    assert_eq!(
        fx.head(Path::new(&path)),
        first_head,
        "nothing new on origin/<B>: the reused alias stays where it was"
    );
    assert!(
        !fx.local_branch_exists(&alias(2)),
        "a rerun must not create a second alias"
    );
    assert_info_line(&second, &fx.caller, &alias(1));
}

/// The planner's remedy for unpushed caller commits is "push them, then
/// re-run". On the re-run the reused alias must contain those commits, so it
/// is fast-forwarded to the origin/<B> step 4 has just fetched.
#[test]
fn rerun_fast_forwards_reused_alias_to_origin() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let first = fx.step4(BRANCH, &[]);
    assert_eq!(first.code, Some(0), "first run\n{}", first.describe());
    let path = worktree_path(&first);
    let pushed = push_new_commit_from_caller(&fx);

    let second = fx.step4(BRANCH, &[]);

    assert_eq!(second.code, Some(0), "rerun\n{}", second.describe());
    assert_eq!(worktree_path(&second), path, "the alias worktree is reused");
    assert_eq!(
        fx.head(Path::new(&path)),
        pushed,
        "the reused alias must be at the commit pushed between runs"
    );
    assert_eq!(
        fx.origin_sha(BRANCH).as_deref(),
        Some(pushed.as_str()),
        "step 4 must not move origin/<B>"
    );
    fx.assert_no_alias_on_origin("the reuse path pushes nothing");
}

/// A reused alias with leftovers from an earlier run stops step 4 rather
/// than letting `git add -A` put them in this run's commit.
#[test]
fn rerun_stops_when_reused_alias_is_dirty() {
    for (case, rel) in [
        ("untracked file", "left-by-first-run.txt"),
        ("tracked change", "feature.txt"),
    ] {
        let fx = Fx::new();
        fx.on_pushed_branch();
        let first = fx.step4(BRANCH, &[]);
        assert_eq!(
            first.code,
            Some(0),
            "{case}: first run\n{}",
            first.describe()
        );
        let path = worktree_path(&first);
        fx.write_in(Path::new(&path), rel, "left by an earlier run\n");
        let pushed = push_new_commit_from_caller(&fx);

        let second = fx.step4(BRANCH, &[]);

        assert_eq!(
            second.code,
            Some(1),
            "{case}: must stop\n{}",
            second.describe()
        );
        assert!(
            second.stdout.trim().is_empty(),
            "{case}: a stop prints nothing on stdout\n{}",
            second.describe()
        );
        assert!(
            second
                .stderr_line_with(&[
                    "ERROR:",
                    &alias(1),
                    "uncommitted changes or untracked files",
                    "#1762"
                ])
                .is_some(),
            "{case}: the ERROR line must name the alias and the cause\n{}",
            second.describe()
        );
        assert_eq!(
            fs::read_to_string(Path::new(&path).join(rel)).unwrap(),
            "left by an earlier run\n",
            "{case}: the leftover is not discarded"
        );
        assert_ne!(
            fx.head(Path::new(&path)),
            pushed,
            "{case}: a dirty alias is not moved"
        );
    }
}

/// A reused alias holding commits origin/<B> lacks (an earlier run committed
/// but did not push) stops step 4: the alias would not be at the caller's
/// commit and step 15 would later fail as diverged.
#[test]
fn rerun_stops_when_reused_alias_is_ahead_of_origin() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let first = fx.step4(BRANCH, &[]);
    assert_eq!(first.code, Some(0), "first run\n{}", first.describe());
    let path = worktree_path(&first);
    fx.commit_in(
        Path::new(&path),
        "unpushed.txt",
        "u\n",
        "earlier run, not pushed",
    );
    let alias_head = fx.head(Path::new(&path));

    let second = fx.step4(BRANCH, &[]);

    assert_eq!(second.code, Some(1), "must stop\n{}", second.describe());
    assert!(second.stdout.trim().is_empty(), "{}", second.describe());
    assert!(
        second
            .stderr_line_with(&[
                "ERROR:",
                &alias(1),
                &format!("origin/{BRANCH} does not"),
                "#1762"
            ])
            .is_some(),
        "the ERROR line must name the alias and the unpushed commits\n{}",
        second.describe()
    );
    assert_eq!(
        fx.head(Path::new(&path)),
        alias_head,
        "the alias's commits are kept"
    );
}

/// Without the reuse helper the alias cannot be checked, so step 4 stops
/// rather than reuse it unchecked.
#[test]
fn rerun_stops_when_reuse_helper_is_missing() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let first = fx.step4(BRANCH, &[]);
    assert_eq!(first.code, Some(0), "first run\n{}", first.describe());
    let path = worktree_path(&first);
    let first_head = fx.head(Path::new(&path));
    push_new_commit_from_caller(&fx);
    let home = fx.bundle_without("workflow_alias_reuse.sh");

    let second = fx.step4(BRANCH, &[("AMPLIHACK_HOME", home.to_str().unwrap())]);

    assert_eq!(second.code, Some(1), "must stop\n{}", second.describe());
    assert!(second.stdout.trim().is_empty(), "{}", second.describe());
    assert!(
        second
            .stderr_line_with(&[
                "ERROR:",
                "workflow_alias_reuse.sh not found",
                &alias(1),
                "#1762"
            ])
            .is_some(),
        "the missing helper must be named\n{}",
        second.describe()
    );
    assert_eq!(
        fx.head(Path::new(&path)),
        first_head,
        "the alias is not moved"
    );
}

/// D5: an alias-shaped branch that fails the reuse conditions is skipped and
/// left untouched, and its `n` is not taken.
#[test]
fn alias_shaped_branch_with_other_upstream_is_skipped() {
    for (case, upstream) in [("another upstream", "main"), ("no worktree", BRANCH)] {
        let fx = Fx::new();
        fx.on_pushed_branch();
        let skipped = alias(1);
        fx.git(
            &fx.caller,
            &[
                "branch",
                "-q",
                "--track",
                &skipped,
                &format!("origin/{upstream}"),
            ],
        );
        let skipped_sha = fx.git(&fx.caller, &["rev-parse", &skipped]);

        let run = fx.step4(BRANCH, &[]);

        assert_eq!(run.code, Some(0), "{case}\n{}", run.describe());
        assert_eq!(
            run.json()["local_branch"],
            alias(2).as_str(),
            "{case}: the next free n is used\n{}",
            run.describe()
        );
        assert_eq!(
            fx.git(&fx.caller, &["rev-parse", &skipped]),
            skipped_sha,
            "{case}: the skipped branch is not moved"
        );
        assert_eq!(
            fx.config_get(&format!("branch.{skipped}.merge")),
            format!("refs/heads/{upstream}"),
            "{case}: the skipped branch keeps its upstream"
        );
        assert!(
            !fx.caller.join("worktrees").join(&skipped).exists(),
            "{case}: the skipped branch is not checked out"
        );
    }
}

/// D5: a file or directory already at `<main>/worktrees/<alias>` is never
/// handed to the remove-and-re-add path; the create pass moves on.
#[test]
fn existing_path_at_alias_location_is_left_alone() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let occupied = fx.caller.join("worktrees").join(alias(1));
    fx.write_in(&occupied, "keep.txt", "not a worktree\n");

    let run = fx.step4(BRANCH, &[]);

    assert_eq!(run.code, Some(0), "{}", run.describe());
    assert_eq!(run.json()["local_branch"], alias(2).as_str());
    assert_eq!(
        fs::read_to_string(occupied.join("keep.txt")).unwrap(),
        "not a worktree\n",
        "an unrelated directory at the alias path must survive"
    );
}

/// D21: a caller strictly behind origin/<B> passes; the alias starts at
/// origin/<B> and the caller's branch is not moved.
#[test]
fn caller_behind_origin_alias_starts_at_origin() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let old = fx.head(&fx.caller);
    let tree = fx.git(&fx.caller, &["rev-parse", "HEAD^{tree}"]);
    let newer = fx
        .git(
            &fx.caller,
            &[
                "commit-tree",
                tree.trim(),
                "-p",
                &old,
                "-m",
                "pushed from elsewhere",
            ],
        )
        .trim()
        .to_owned();
    fx.git(
        &fx.caller,
        &[
            "push",
            "-q",
            "origin",
            &format!("{newer}:refs/heads/{BRANCH}"),
        ],
    );
    // The caller has not fetched since: only step 4's own fetch shows it.
    fx.git(
        &fx.caller,
        &["update-ref", &format!("refs/remotes/origin/{BRANCH}"), &old],
    );

    let run = fx.step4(BRANCH, &[]);

    assert_eq!(run.code, Some(0), "{}", run.describe());
    let path = worktree_path(&run);
    assert_eq!(
        fx.head(Path::new(&path)),
        newer,
        "the alias starts at origin/<B>"
    );
    assert_eq!(
        fx.head(&fx.caller),
        old,
        "the caller's checkout is not moved"
    );
    assert_eq!(
        fx.git(&fx.caller, &["rev-parse", &format!("refs/heads/{BRANCH}")])
            .trim(),
        old,
        "the caller's branch is not fast-forwarded"
    );
}

/// Gate B with a linked-worktree caller (D6): the alias is anchored at the
/// main repository, and the INFO line names the linked worktree.
#[test]
fn linked_worktree_caller_gets_alias_under_main_repo() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    fx.git(&fx.caller, &["switch", "-q", "main"]);
    let linked = fx.root.join("linked");
    fx.git(
        &fx.caller,
        &["worktree", "add", "-q", linked.to_str().unwrap(), BRANCH],
    );
    let before = fx.caller_state(&linked);

    let run = fx.step4(BRANCH, &[("REPO_PATH", linked.to_str().unwrap())]);

    assert_eq!(run.code, Some(0), "{}", run.describe());
    assert_eq!(
        worktree_path(&run),
        fx.caller.join("worktrees").join(alias(1)).to_str().unwrap(),
        "aliases live under the main repository (#1134)"
    );
    assert_info_line(&run, &linked, &alias(1));
    assert!(
        run.stderr_line_with(&["INFO:", "#1762", &format!("at {};", linked.display())])
            .is_some(),
        "the caller path in the INFO line is the linked worktree\n{}",
        run.describe()
    );
    assert_eq!(
        fx.caller_state(&linked),
        before,
        "the linked caller is untouched"
    );
}

/// The issue's third case: a caller that is not on <B> behaves as before.
#[test]
fn caller_not_on_branch_is_unchanged() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    fx.git(&fx.caller, &["switch", "-q", "main"]);

    let run = fx.step4(BRANCH, &[]);

    assert_eq!(run.code, Some(0), "{}", run.describe());
    let json = run.json();
    assert_eq!(json["branch_name"], BRANCH);
    assert!(
        json.get("local_branch").is_none() && json.get("local_branch_reason").is_none(),
        "local_branch and local_branch_reason appear only on the alias path: {json}"
    );
    let path = worktree_path(&run);
    assert_eq!(
        path,
        fx.caller.join("worktrees").join(BRANCH).to_str().unwrap()
    );
    assert_eq!(
        fx.git(Path::new(&path), &["symbolic-ref", "--short", "HEAD"])
            .trim(),
        BRANCH
    );
    assert!(
        fx.alias_branches().is_empty(),
        "no alias off the alias path"
    );
    assert!(
        !run.stderr.contains("#1762"),
        "nothing about #1762 off the alias path\n{}",
        run.describe()
    );
}
