//! Issue #2716: a traced task that exits while the observer holds it stopped
//! must not fail the run. On 1.2.167 eight recipe runs on one host ended with
//! "failed to wait for recipe-runner-rs: required process observation failed:
//! No such process (os error 3)", were classed indeterminate, and so were
//! never retried. `ESRCH` from a ptrace request on a stopped tracee is the
//! documented result when that tracee died between the stop and the request,
//! which is ordinary in a tree of agent, node, cargo and shell processes.
//!
//! Each test runs the real `amplihack recipe run` with the Rust boundary
//! fixture as its runner, under the real observer, in a mode that makes a
//! traced task disappear inside the observer's stop-to-resume window:
//!
//! - `exit-group-thread`: a thread group whose leader calls `exit_group`
//!   while a freshly cloned thread of that group is still at its initial
//!   ptrace stop.
//! - `sigkill-child`: a child killed with SIGKILL between its stop and the
//!   continue that would have resumed it.
//!
//! Both race the clone-newborn resume and the loop-tail resume, never the
//! exec stop: the two `/proc` reads at an exec stop are deliberately still
//! fatal, so a test that raced them would be shaped to dodge a live defect.
//!
//! The run's reported status must be the runner's own exit. All state stays
//! under the fixture's tempdir home; the real `~/.claude` and `~/.amplihack`
//! are never read or written, and the fixture stops only a pid it created
//! and recorded. No sleep, timer, retry or poll appears here or in the
//! fixture: the races are made to land by bounded repetition alone, and a
//! race that does not land fails the test instead of passing it.

#![cfg(target_os = "linux")]

use super::{contract_support as boundary, *};

/// What the run itself recorded, read from its own persisted report.
fn report(f: &Fixture) -> serde_json::Value {
    serde_json::from_slice(&fs::read(boundary::run_dir(f).join("run.json")).unwrap()).unwrap()
}

/// Runs the boundary fixture in `mode` under the real observer and asserts
/// the one thing this issue is about: the run's status follows the runner.
fn a_vanished_task_leaves_the_run_alone(label: &str, mode: &str) {
    let f = Fixture::new();
    let output = boundary::command(&f, label)
        .env("ISSUE_2096_MODE", mode)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // The exact text the eight field failures ended with, named before the
    // status assertion so a reproduction reads as itself rather than as a
    // bare non-zero exit.
    for phrase in [
        "required process observation failed",
        "required observer did not finalize",
        "observer lost runner exit status",
    ] {
        assert!(
            !stderr.contains(phrase),
            "a task that had gone ended the run with {phrase:?}:\n{stderr}"
        );
    }
    succeeds(&output);

    let report = report(&f);
    assert_eq!(
        report["outcome"], "success",
        "the runner finished, so the run's status is the runner's: {report}"
    );
    // `indeterminate` is a failure class, not an outcome: it reaches a
    // caller as this stderr marker. A run that follows its runner's own
    // exit never writes one, so nothing downstream can decline to retry a
    // run that did not fail.
    assert!(
        !stderr.contains("amplihack.recipe.failure_class"),
        "the run classified a failure it did not have:\n{stderr}"
    );

    // The reproduction only means something if the run was observed at all:
    // a host whose ptrace policy forbids that fails here instead of passing
    // vacuously.
    assert_eq!(
        report["git_recording"]["coverage"], "established",
        "the run was not observed; this test needs ptrace (Yama ptrace_scope <= 1): {report}"
    );

    // Non-vacuity. The receipt that counts the tasks found gone is removed
    // with the trace folder when recording finalizes, so the observer's own
    // warning on stderr is what proves the race landed.
    //
    // If it did not, the iteration count is the wrong thing to reach for: a
    // mode lands because of how it is built, not how often it repeats. Check
    // that the cloning is still done by freely running threads and that
    // `sigkill-child` still signals the *previous* child. Never add a sleep,
    // timer, retry or poll.
    assert!(
        stderr.contains("was gone at"),
        "no task went away, so {mode} proved nothing; check the mode's \
         structure before its iteration count:\n{stderr}"
    );
}

#[test]
fn a_thread_destroyed_by_exit_group_does_not_fail_the_run() {
    a_vanished_task_leaves_the_run_alone("vanished-exit-group", "exit-group-thread");
}

#[test]
fn a_child_killed_in_its_stop_does_not_fail_the_run() {
    a_vanished_task_leaves_the_run_alone("vanished-sigkill", "sigkill-child");
}
