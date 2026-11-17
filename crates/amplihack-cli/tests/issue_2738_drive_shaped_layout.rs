//! Issue #2738: a recipe run works in a drive-shaped layout.
//!
//! An auto-drive works in a clone that holds its pull-request worktrees under
//! `worktrees/pr-N`. PR #2290 recorded that its own observer stopped
//! `git -C <main root> worktree list` in exactly that layout, which is how a
//! step finds the drive's main root. The layout is rebuilt here: a bare
//! origin, a clone of it, and a `worktrees/pr-7` worktree on its own branch.
//!
//! The run is started from the clone's main root, and the step asks the same
//! question from inside the run. The run reaches a terminal outcome, the
//! question is answered, and the worktree is still registered afterwards.

#![cfg(unix)]

use crate::issue_2738_support::{ATTRIBUTION_MESSAGE, Fixture, succeeds};

#[test]
fn a_clone_with_a_pr_worktree_runs_and_keeps_its_worktree() {
    let f = Fixture::new();
    let worktree = f.add_worktree("pr-7");
    assert!(worktree.join("tracked").is_file());
    let before = f.worktrees();
    assert!(
        before.contains(worktree.to_str().unwrap()),
        "the fixture did not register the worktree:\n{before}"
    );

    let out = Fixture::finish(
        "drive",
        f.command("drive").env("ISSUE_2738_WORKTREE_LIST", &f.repo),
    );
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !stderr.contains(ATTRIBUTION_MESSAGE),
        "the run must not be ended by Git attribution:\n{stderr}"
    );
    succeeds(&out);

    // The step's own `git -C <main root> worktree list` answered.
    assert_eq!(
        f.observed_str("drive", "caller_worktrees.status"),
        "0",
        "git worktree list failed inside the run:\n{}",
        f.observed_str("drive", "caller_worktrees.err")
    );
    let listed = f.observed_str("drive", "caller_worktrees");
    assert!(
        listed.contains(worktree.to_str().unwrap()),
        "the step did not see the drive's pull-request worktree:\n{listed}"
    );

    // The caller's worktree record is as it was, and the worktree still works.
    assert_eq!(f.worktrees(), before, "the worktree record changed");
    assert!(
        worktree.join("tracked").is_file(),
        "the pull-request worktree lost its content"
    );
}

#[test]
fn a_run_started_from_a_pr_worktree_finishes() {
    let f = Fixture::new();
    let worktree = f.add_worktree("pr-9");
    let before = f.worktrees();

    let out = Fixture::finish(
        "from-worktree",
        &mut f.command_in("from-worktree", &worktree),
    );
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !stderr.contains(ATTRIBUTION_MESSAGE),
        "the run must not be ended by Git attribution:\n{stderr}"
    );
    succeeds(&out);

    let cwd = f.observed_str("from-worktree", "cwd");
    assert_ne!(
        std::path::Path::new(&cwd),
        worktree.as_path(),
        "the run must not work in the pull-request worktree itself"
    );
    assert_eq!(f.worktrees(), before, "the worktree record changed");
}
