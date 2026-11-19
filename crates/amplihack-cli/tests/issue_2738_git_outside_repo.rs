//! Issue #2738 / #2737: a Git command run outside any repository while a
//! recipe run is in flight does not affect the run.
//!
//! The tracer added by #2290 attributed every Git command it observed to a
//! repository and an index by asking `git rev-parse --absolute-git-dir` where
//! the command started. A `git clone` or a `git ls-remote` in a directory
//! that is not a repository has no such answer, so the attribution failed and
//! the run ended with "cannot attribute an observed Git command to its
//! repository/index". On 16 November 2026 that ended 24 of 74 auto-drives on
//! one host. Nothing attributes Git commands now, so neither command can
//! reach the run at all.
//!
//! The remote is a bare repository in the test's own tempfile root: no
//! network, no credential helper, no real host.

#![cfg(unix)]

use crate::issue_2738_support::{ATTRIBUTION_MESSAGE, Fixture, succeeds};
use std::path::Path;

#[test]
fn a_git_clone_and_a_git_ls_remote_outside_any_repository_do_not_affect_the_run() {
    let f = Fixture::new();
    let outside = f.root.join("outside");
    let before = f.snapshot();

    let out = Fixture::finish(
        "outside",
        f.command("outside")
            .env("ISSUE_2738_OUTSIDE", &outside)
            .env("ISSUE_2738_REMOTE", &f.origin),
    );

    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !stderr.contains(ATTRIBUTION_MESSAGE),
        "the run must not be ended by Git attribution:\n{stderr}"
    );
    succeeds(&out);

    // Both commands ran, outside any repository, and both succeeded.
    assert!(
        !outside.join(".git").exists(),
        "the directory the commands ran in must not be a repository"
    );
    assert_eq!(
        f.observed_str("outside", "ls_remote.status"),
        "0",
        "git ls-remote failed:\n{}",
        f.observed_str("outside", "ls_remote.err")
    );
    assert_eq!(
        f.observed_str("outside", "clone.status"),
        "0",
        "git clone failed:\n{}",
        f.observed_str("outside", "clone.err")
    );
    assert!(
        outside.join("clone/tracked").is_file(),
        "the clone made during the run is missing its content"
    );
    assert!(
        f.observed_str("outside", "ls_remote.out")
            .contains("refs/heads/main"),
        "git ls-remote did not list the remote's branches"
    );

    // Git outside the repository leaves the selected checkout unchanged.
    let cwd = f.observed_str("outside", "cwd");
    assert_eq!(Path::new(&cwd), f.repo.as_path());
    assert_eq!(f.snapshot(), before, "the caller's checkout is unchanged");
}
