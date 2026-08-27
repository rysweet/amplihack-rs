// crates/amplihack-cli/src/commands/recipe/run/tests_terminal_record.rs
//
// Issue #1304 — every exit from the recipe runner must leave a DURABLE
// TERMINAL RECORD behind.
//
// Observed bug: a smart-orchestrator run exited 1 after ~12 minutes having
// emitted no failed step, no error, and no final `amplihack.recipe.log_pointer`.
// From outside, a run that died is indistinguishable from a run that never
// started: the operator gets a bare `exit 1`, no run id to search the logs
// for, and nothing naming the runner or its working tree.
//
// Two defects produced that:
//
//   1. Failure paths discarded the correlation summary (`let _summary = ...`),
//      so a FAILING run told the operator strictly LESS than a succeeding one,
//      which returns the same detail in `result.log_pointer`.
//   2. On the timeout path `terminate_recipe_runner(...)?` ran BEFORE
//      `emit_final`, so a timeout whose cleanup also failed emitted no record
//      at all -- the worst case reported the least.
//
// These tests pin the contract: whatever goes wrong, the error names the run.
#![cfg(unix)]

use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const RUNNER_PATH_ENV: &str = "RECIPE_RUNNER_RS_PATH";
const SESSION_TREE_DIR_ENV: &str = "AMPLIHACK_SESSION_TREE_DIR";
const RUNNER_TIMEOUT_ENV: &str = "AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS";

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

fn write_stub(path: &Path, body: &str) {
    std::fs::write(path, body).expect("failed to write runner stub");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("failed to chmod runner stub");
}

/// Every `anyhow` context layer, joined -- this is what the operator actually
/// reads when the command fails.
fn error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// A run id is a uuid; asserting on the literal value would be asserting on
/// randomness. What matters is that the error names *a* run.
fn names_a_run(text: &str) -> bool {
    text.contains("recipe run ") && text.contains("status=")
}

// -------------------------------------------------------------------------
// A runner that exits non-zero with unparseable stdout is the exact shape of
// the reported failure: the process died having said nothing structured.
// -------------------------------------------------------------------------
#[test]
fn a_runner_that_dies_without_a_result_still_names_the_run() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    write_stub(&runner, "#!/bin/sh\necho 'not json at all'\nexit 1\n");

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: dies-silently\nsteps: []\n").expect("failed to write recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _tree_env = EnvVarGuard::set(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    let _timeout_env = EnvVarGuard::unset(RUNNER_TIMEOUT_ENV);

    let error = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false,
        true,
        temp.path(),
        &[],
        None,
    )
    .expect_err("a runner exiting 1 with unparseable output must be an error");

    let chain = error_chain(&error);
    assert!(
        names_a_run(&chain),
        "a failing run must name its run id and status so the operator can find \
         the logs; issue #1304. Got: {chain}"
    );
}

// -------------------------------------------------------------------------
// The timeout path. `terminate_recipe_runner(...)?` used to run BEFORE
// `emit_final`, so a timeout whose cleanup also failed emitted no record at
// all. The record now comes first, and the timeout error names the run.
// -------------------------------------------------------------------------
#[test]
fn a_runner_that_times_out_still_names_the_run() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    // Sleeps well past the timeout below, so the wait returns None.
    write_stub(&runner, "#!/bin/sh\n/bin/sleep 120\n");

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: hangs\nsteps: []\n").expect("failed to write recipe");

    let _runner_env = EnvVarGuard::set(RUNNER_PATH_ENV, &runner);
    let _tree_env = EnvVarGuard::set(SESSION_TREE_DIR_ENV, temp.path().join("trees"));
    let _timeout_env = EnvVarGuard::set(RUNNER_TIMEOUT_ENV, "1");

    let error = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false,
        true,
        temp.path(),
        &[],
        None,
    )
    .expect_err("a runner that never exits must time out");

    let chain = error_chain(&error);
    assert!(
        chain.contains("timed out"),
        "a timeout must still say it timed out. Got: {chain}"
    );
    assert!(
        names_a_run(&chain),
        "a timed-out run must name its run id and status; the record is now \
         emitted before teardown can fail and swallow it. Issue #1304. \
         Got: {chain}"
    );
}
