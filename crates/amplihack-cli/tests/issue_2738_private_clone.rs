//! Issue #2738, keeping #2096: each run works in its own private clone, never
//! the caller's checkout.
//!
//! This is the part of #2290 that was asked for and stays. It is asserted
//! here on its own, so that removing the Git-command tracer cannot take the
//! private clone's coverage with it: a step that stages and commits must
//! change only the run's own checkout, and the caller's index, `HEAD`,
//! tracked files and configuration must be exactly as they were.

#![cfg(unix)]

use crate::issue_2738_support::{Fixture, succeeds};
use std::path::Path;

#[test]
fn the_run_works_in_a_private_clone_and_a_step_that_commits_leaves_the_caller_alone() {
    let f = Fixture::new();
    let before = f.snapshot();

    let out = f
        .command("private")
        .env("ISSUE_2738_STAGE", "1")
        .output()
        .unwrap();
    succeeds(&out);

    let cwd = f.observed_str("private", "cwd");
    let checkout = Path::new(&cwd);
    assert_eq!(
        f.observed_str("private", "startup_cwd"),
        cwd,
        "the runner must already be in the private checkout when it starts"
    );
    assert_ne!(
        checkout,
        f.repo.as_path(),
        "the run must not work in the caller's checkout"
    );
    assert!(
        !checkout.starts_with(&f.repo),
        "the run's checkout must not be inside the caller's checkout"
    );
    assert_eq!(
        f.observed_str("private", "run_checkout"),
        cwd,
        "AMPLIHACK_RUN_CHECKOUT must name the private checkout"
    );
    assert_eq!(
        f.observed("private", "head"),
        f.git(&["rev-parse", "HEAD"]),
        "the private checkout starts at the caller's HEAD"
    );
    assert!(
        f.observed("private", "branch").is_empty(),
        "the private checkout must be detached"
    );

    // The caller carries none of the step's work.
    assert_eq!(f.snapshot(), before, "the caller's checkout is read-only");
    assert!(
        !f.repo.join("run-output").exists(),
        "the step's file reached the caller's checkout"
    );
    assert!(
        !checkout.exists(),
        "the run's checkout {cwd} was left behind"
    );
}

/// The same guarantee for a commit whose pre-commit hook is itself a Git-running
/// descendant of the step. Carried over from the recording coverage removed in
/// #2738: a descendant that Git did not launch directly still works in the run's
/// own checkout, not the caller's.
#[test]
fn a_hook_descendant_that_commits_leaves_the_callers_checkout_alone() {
    let f = Fixture::new();
    let before = f.snapshot();

    let out = f
        .command("hook")
        .env("ISSUE_2738_HOOK", "1")
        .output()
        .unwrap();
    succeeds(&out);

    assert_eq!(
        f.observed("hook", "hook_committed"),
        b"hook descendant\n",
        "the hook's own file must be in the run's commit"
    );
    assert_ne!(
        Path::new(&f.observed_str("hook", "cwd")),
        f.repo.as_path(),
        "the hook-driven commit must not have run in the caller's checkout"
    );
    assert_eq!(f.snapshot(), before, "the caller's checkout is read-only");
    assert!(
        !f.repo.join("hook-output").exists(),
        "the hook's file reached the caller's checkout"
    );
}
