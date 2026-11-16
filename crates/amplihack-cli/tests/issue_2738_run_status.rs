//! Issue #2738: a recipe run's status is the runner's own.
//!
//! The tracer removed in this change could overrule the runner. A Git command
//! it observed but could not attribute to a repository and an index ended the
//! run whatever the runner had done (#2737), and on a host that denies
//! `ptrace` the run could not start at all (#2716). Both were the tracer
//! speaking, not the runner.
//!
//! The three other behaviour files assert the success direction. These two
//! assert the failure direction, which is where an overruling tracer would
//! have shown: a runner that ends badly must be the reason the run ends
//! badly, and the run must say so rather than say anything about Git.
//!
//! Both run the stub runner from `issue_2738_support`, inside that fixture's
//! single tempfile root.

#![cfg(unix)]

use crate::issue_2738_support::{ATTRIBUTION_MESSAGE, Fixture};

/// A runner that exits non-zero without a report: the run fails, and it fails
/// for the runner's reason.
#[test]
fn a_runner_that_exits_non_zero_ends_the_run_and_nothing_else_does() {
    let f = Fixture::new();
    let before = f.snapshot();

    let out = Fixture::finish(
        "runner-exit",
        f.command("runner-exit")
            .env("ISSUE_2738_RUNNER_EXIT", "3")
            // One attempt, so the assertion is about this exit and not about
            // a later one: the transient-retry bound is read from the
            // environment.
            .env("AMPLIHACK_RECIPE_TRANSIENT_MAX_ATTEMPTS", "1"),
    );

    // The runner was reached: the failure is its own, not a launch failure.
    assert!(
        !f.observed_str("runner-exit", "startup_cwd").is_empty(),
        "the runner never started, so its exit is not what ended the run"
    );
    assert!(
        !out.status.success(),
        "the run reported success over a runner that exited 3\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !said.contains(ATTRIBUTION_MESSAGE),
        "the run blamed Git attribution:\n{said}"
    );
    assert!(
        !said.contains("ptrace") && !said.contains("git-recording"),
        "the run blamed the removed tracer:\n{said}"
    );
    assert_eq!(f.snapshot(), before, "the caller's checkout is unchanged");
}

/// A runner that exits zero but reports its own failure: the run follows the
/// report it was given.
#[test]
fn a_runner_that_reports_its_own_failure_ends_the_run() {
    let f = Fixture::new();

    let out = Fixture::finish(
        "runner-reports-failure",
        f.command("runner-reports-failure")
            .env("ISSUE_2738_RUNNER_REPORTS_FAILURE", "1")
            .env("AMPLIHACK_RECIPE_TRANSIENT_MAX_ATTEMPTS", "1"),
    );

    assert!(
        !f.observed_str("runner-reports-failure", "startup_cwd")
            .is_empty(),
        "the runner never started"
    );
    assert!(
        !out.status.success(),
        "the run reported success over a runner that reported failure\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !said.contains(ATTRIBUTION_MESSAGE),
        "the run blamed Git attribution:\n{said}"
    );
}
