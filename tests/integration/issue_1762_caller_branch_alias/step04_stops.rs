//! Step 4's stops on the alias path (#1762, D4, D7, R15, D16): the caller's
//! checkout holds something the task worktree would not see, the branch is
//! the default branch, the planner is missing or answers badly, or a
//! concurrent run took the alias path first. Every stop exits non-zero with
//! nothing on stdout, creates no alias, and leaves the caller as it was.

use std::fs;

use super::fixture::*;

/// The D7 stop contract for `cause`: exit 1, no stdout, one stderr line naming
/// the branch, the caller's path, the cause, "would not see" and #1762, and
/// nothing created.
fn assert_stop(fx: &Fx, run: &Run, cause: &str, why: &str) {
    assert_eq!(
        run.code,
        Some(1),
        "{why}: must stop with exit 1\n{}",
        run.describe()
    );
    assert!(
        run.stdout.trim().is_empty(),
        "{why}: a stop prints nothing on stdout\n{}",
        run.describe()
    );
    let quoted = format!("'{BRANCH}'");
    let caller = fx.caller.to_str().unwrap();
    assert!(
        run.stderr_line_with(&["ERROR:", &quoted, caller, cause, "would not see", "#1762"])
            .is_some(),
        "{why}: one ERROR line must name {quoted}, {caller}, `{cause}`, \"would not see\" and #1762\n{}",
        run.describe()
    );
    assert!(
        fx.alias_branches().is_empty(),
        "{why}: no alias branch is created"
    );
    let worktrees = fx.git(&fx.caller, &["worktree", "list", "--porcelain"]);
    assert_eq!(
        worktrees
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        1,
        "{why}: no task worktree is created\n{worktrees}"
    );
}

#[test]
fn stops_on_uncommitted_tracked_change() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    fx.write_in(&fx.caller, "feature.txt", "edited, not committed\n");
    let before = fx.caller_state(&fx.caller);

    let run = fx.step4(BRANCH, &[]);

    assert_stop(&fx, &run, "uncommitted changes", "dirty tracked file");
    assert_eq!(
        fx.caller_state(&fx.caller),
        before,
        "the caller's work is kept"
    );
    assert_eq!(
        fs::read_to_string(fx.caller.join("feature.txt")).unwrap(),
        "edited, not committed\n"
    );
}

#[test]
fn stops_when_branch_not_on_origin() {
    let fx = Fx::new();
    fx.git(&fx.caller, &["switch", "-q", "-c", BRANCH]);
    fx.commit_in(&fx.caller, "local.txt", "local\n", "never pushed");
    let before = fx.caller_state(&fx.caller);

    let run = fx.step4(BRANCH, &[]);

    assert_stop(
        &fx,
        &run,
        &format!("origin/{BRANCH} does not exist"),
        "branch never pushed",
    );
    assert!(
        run.stderr.contains(&format!("git push -u origin {BRANCH}")),
        "the stop names the remedy\n{}",
        run.describe()
    );
    assert_eq!(fx.caller_state(&fx.caller), before);
    assert_eq!(
        fx.origin_sha(BRANCH),
        None,
        "step 4 never pushes <B> for the caller"
    );
}

#[test]
fn stops_on_unpushed_commit() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let pushed = fx.head(&fx.caller);
    fx.commit_in(&fx.caller, "one.txt", "1\n", "unpushed one");
    fx.commit_in(&fx.caller, "two.txt", "2\n", "unpushed two");
    let before = fx.caller_state(&fx.caller);

    let run = fx.step4(BRANCH, &[]);

    assert_stop(&fx, &run, "has 2 commit(s)", "two unpushed commits");
    assert!(
        run.stderr.contains(&format!("not on origin/{BRANCH}")),
        "{}",
        run.describe()
    );
    assert_eq!(
        fx.caller_state(&fx.caller),
        before,
        "the commits stay in the caller"
    );
    assert_eq!(fx.origin_sha(BRANCH).as_deref(), Some(pushed.as_str()));
}

/// AC5 to AC7: only the first failing check is reported.
#[test]
fn reports_only_first_cause() {
    // Dirty and ahead of origin/<B>: check 1 wins over check 3.
    let fx = Fx::new();
    fx.on_pushed_branch();
    fx.commit_in(&fx.caller, "ahead.txt", "ahead\n", "unpushed");
    fx.write_in(&fx.caller, "feature.txt", "dirty\n");
    let run = fx.step4(BRANCH, &[]);
    assert_stop(&fx, &run, "uncommitted changes", "dirty and ahead");
    assert!(
        !run.stderr.contains("commit(s) that are not on"),
        "{}",
        run.describe()
    );

    // Dirty and never pushed: check 1 wins over check 2.
    let fx = Fx::new();
    fx.git(&fx.caller, &["switch", "-q", "-c", BRANCH]);
    fx.write_in(&fx.caller, "README.md", "dirty\n");
    let run = fx.step4(BRANCH, &[]);
    assert_stop(&fx, &run, "uncommitted changes", "dirty and never pushed");
    assert!(!run.stderr.contains("does not exist"), "{}", run.describe());

    assert_eq!(
        run.stderr
            .lines()
            .filter(|l| l.contains("would not see"))
            .count(),
        1,
        "exactly one cause is reported\n{}",
        run.describe()
    );
}

/// The default branch never gets an alias; the #858 refusal stands. `trunk`
/// is the default only through refs/remotes/origin/HEAD.
#[test]
fn default_branch_keeps_858_refusal() {
    for base in ["main", "trunk"] {
        let fx = Fx::new();
        if base != "main" {
            fx.git(&fx.caller, &["switch", "-q", "-c", base]);
            fx.git(&fx.caller, &["push", "-q", "-u", "origin", base]);
            fx.git(&fx.caller, &["remote", "set-head", "origin", base]);
        }

        let run = fx.step4(base, &[]);

        assert_eq!(run.code, Some(1), "{base}\n{}", run.describe());
        assert!(run.stdout.trim().is_empty(), "{base}\n{}", run.describe());
        assert!(
            run.stderr.contains("refusing to use the caller checkout")
                && run.stderr.contains("issue #858"),
            "{base}: the #858 refusal is kept for the default branch\n{}",
            run.describe()
        );
        let aliases: Vec<String> = fx
            .git(
                &fx.caller,
                &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
            )
            .lines()
            .filter(|l| l.contains("--wt-"))
            .map(str::to_owned)
            .collect();
        assert!(aliases.is_empty(), "{base}: no alias, got {aliases:?}");
    }
}

/// R15/D7: with the planner unreachable, step 4 warns and then refuses as it
/// did before #1762.
#[test]
fn missing_planner_warns_then_refuses() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let empty = fx.root.join("no-bundle");
    fs::create_dir_all(&empty).unwrap();

    let run = fx.step4(BRANCH, &[("AMPLIHACK_HOME", empty.to_str().unwrap())]);

    assert_eq!(run.code, Some(1), "{}", run.describe());
    assert!(run.stdout.trim().is_empty(), "{}", run.describe());
    let warning = run
        .stderr
        .lines()
        .position(|l| {
            l.contains("WARNING:")
                && l.contains(&format!("{PLANNER} not found"))
                && l.contains(&format!("'{BRANCH}'"))
                && l.contains("#1762")
        })
        .unwrap_or_else(|| panic!("the missing planner must be named\n{}", run.describe()));
    let refusal = run
        .stderr
        .lines()
        .position(|l| l.contains("refusing to use the caller checkout") && l.contains("issue #858"))
        .unwrap_or_else(|| panic!("the #858 refusal must follow\n{}", run.describe()));
    assert!(
        warning < refusal,
        "the WARNING comes first\n{}",
        run.describe()
    );
    assert!(fx.alias_branches().is_empty());
}

/// API recommendations 1 and 2: step 4 accepts the planner's answer only on
/// exit 0 with an output that starts with `<B>--wt-`.
#[test]
fn planner_output_is_validated() {
    for (case, body) in [
        ("foreign name", "#!/usr/bin/env bash\necho other-branch\n"),
        ("escaping path", "#!/usr/bin/env bash\necho ../../escape\n"),
        ("exit 1", "#!/usr/bin/env bash\nexit 1\n"),
        ("exit 0, no output", "#!/usr/bin/env bash\nexit 0\n"),
    ] {
        let fx = Fx::new();
        fx.on_pushed_branch();
        let bundle = fx.bundle_with_planner(body);

        let run = fx.step4(BRANCH, &[("AMPLIHACK_HOME", bundle.to_str().unwrap())]);

        assert_eq!(run.code, Some(1), "{case}\n{}", run.describe());
        assert!(run.stdout.trim().is_empty(), "{case}\n{}", run.describe());
        assert!(
            run.stderr_line_with(&[
                "ERROR:",
                "cannot set up an alias worktree",
                &format!("'{BRANCH}'"),
                "#1762",
            ])
            .is_some(),
            "{case}\n{}",
            run.describe()
        );
        assert!(
            !run.stderr.contains("issue #858"),
            "{case}: a present planner replaces the #858 refusal\n{}",
            run.describe()
        );
        for name in ["other-branch", "escape"] {
            assert!(!fx.local_branch_exists(name), "{case}: no branch `{name}`");
        }
        assert!(
            !fx.root.join("escape").exists() && !fx.caller.join("worktrees/other-branch").exists(),
            "{case}: nothing is created from a rejected answer"
        );
    }
}

/// S3/D16: another run took the alias path between the planner and the add.
/// The create is `git worktree add -b` itself, so it fails and the other
/// run's worktree is not removed.
#[test]
fn concurrent_create_does_not_remove_existing_worktree() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let taken = fx.caller.join("worktrees").join(alias(1));
    fx.git(
        &fx.caller,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other-run",
            "--",
            taken.to_str().unwrap(),
            "main",
        ],
    );
    fx.write_in(&taken, "other-run-work.txt", "in progress\n");
    let bundle = fx.bundle_with_planner(
        "#!/usr/bin/env bash\n# Answers as the real planner did before the other run won.\nprintf '%s--wt-1\\n' \"$3\"\n",
    );

    let run = fx.step4(BRANCH, &[("AMPLIHACK_HOME", bundle.to_str().unwrap())]);

    assert_ne!(
        run.code,
        Some(0),
        "the second create must fail\n{}",
        run.describe()
    );
    assert!(run.stdout.trim().is_empty(), "{}", run.describe());
    assert!(
        !run.stderr.contains("issue #858"),
        "the failure comes from the create, not the #858 refusal\n{}",
        run.describe()
    );
    assert_eq!(
        fs::read_to_string(taken.join("other-run-work.txt")).unwrap_or_default(),
        "in progress\n",
        "the other run's worktree and its work must survive"
    );
    assert_eq!(
        fx.git(&taken, &["symbolic-ref", "--short", "HEAD"]).trim(),
        "other-run",
        "the other run's worktree stays on its branch"
    );
}

/// F1: another run has created the alias ref, but its worktree is still
/// locked and not yet on the alias. Step 4 stops instead of removing that
/// directory, and leaves the ref and the lock as they were.
#[test]
fn alias_ref_made_by_other_run_is_not_removed() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let taken = fx.caller.join("worktrees").join(alias(1));
    let taken_str = taken.to_str().unwrap();
    fx.git(
        &fx.caller,
        &[
            "branch",
            "-q",
            "--track",
            &alias(1),
            &format!("origin/{BRANCH}"),
        ],
    );
    fx.git(
        &fx.caller,
        &["worktree", "add", "-q", "--detach", "--", taken_str, "main"],
    );
    fx.git(
        &fx.caller,
        &[
            "worktree",
            "lock",
            "--reason",
            "initializing",
            "--",
            taken_str,
        ],
    );
    fx.write_in(&taken, "other-run-work.txt", "in progress\n");
    let bundle = fx.bundle_with_planner(
        "#!/usr/bin/env bash\n# Answers as the real planner did before the other run won.\nprintf '%s--wt-1\\n' \"$3\"\n",
    );

    let run = fx.step4(BRANCH, &[("AMPLIHACK_HOME", bundle.to_str().unwrap())]);

    assert_ne!(run.code, Some(0), "step 4 must stop\n{}", run.describe());
    assert!(run.stdout.trim().is_empty(), "{}", run.describe());
    assert!(
        run.stderr_line_with(&["ERROR:", "not checked out", "#1762"])
            .is_some(),
        "one ERROR line names the cause and #1762\n{}",
        run.describe()
    );
    assert!(
        !run.stderr.contains("Removing stale worktree"),
        "the other run's worktree is never treated as stale\n{}",
        run.describe()
    );
    assert_eq!(
        fs::read_to_string(taken.join("other-run-work.txt")).unwrap_or_default(),
        "in progress\n",
        "the other run's worktree and its work must survive"
    );
    assert!(
        fx.local_branch_exists(&alias(1)),
        "the other run's alias ref is kept"
    );
    let worktrees = fx.git(&fx.caller, &["worktree", "list", "--porcelain"]);
    assert!(
        worktrees.split("\n\n").any(|entry| {
            let mut lines = entry.lines();
            lines.next() == Some(&format!("worktree {taken_str}"))
                && lines.any(|l| l.starts_with("locked"))
        }),
        "the other run's worktree is still registered and locked\n{worktrees}"
    );
}
