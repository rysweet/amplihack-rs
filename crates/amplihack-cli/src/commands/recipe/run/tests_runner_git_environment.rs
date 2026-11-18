//! Issues #2738/#2808: a caller's Git routing and Trace2 settings do not follow a run.
//!
//! The Git-command tracer removed in #2738 cleared these variables as a side
//! effect of owning its own trace target. command construction now owns
//! them for their own reason, so this asserts the behaviour directly rather
//! than through whatever happens to clear them.
//!
//! `GIT_TRACE2_REDACT` is the one with teeth: Git redacts `Authorization:`
//! headers in Trace2 output by default, and `=0` turns that off. A caller
//! environment carrying a `GIT_TRACE2*` target together with `=0` would
//! otherwise write this run's auth headers to a sink of the caller's
//! choosing.

use super::execute::{CALLER_TRACE_VARS, scrub_runner_git_environment};
use std::ffi::{OsStr, OsString};
use std::process::Command;

/// `Some(None)` means the key is explicitly removed from the child; `None`
/// means the command says nothing about it and the child inherits it.
fn env_of(command: &Command, key: &str) -> Option<Option<OsString>> {
    command
        .get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v.map(OsStr::to_os_string))
}

/// The list is the one git reads, and `GIT_TRACE2_REDACT` is in it.
#[test]
fn the_cleared_list_names_every_trace2_variable_including_redact() {
    for expected in [
        "GIT_TRACE2",
        "GIT_TRACE2_CONFIG_PARAMS",
        "GIT_TRACE2_ENV_VARS",
        "GIT_TRACE2_EVENT",
        "GIT_TRACE2_PERF",
        "GIT_TRACE2_REDACT",
    ] {
        assert!(
            CALLER_TRACE_VARS.contains(&expected),
            "{expected} must be cleared from a run's environment"
        );
    }
}

/// The caller's own checkout is a run like any other: the clearing is unconditional.
#[test]
fn a_caller_checkout_clears_the_callers_trace_settings() {
    let mut command = Command::new("runner");
    for var in CALLER_TRACE_VARS {
        command.env(var, "/caller/chosen/target");
    }
    scrub_runner_git_environment(&mut command);
    for var in CALLER_TRACE_VARS {
        assert_eq!(
            env_of(&command, var),
            Some(None),
            "{var} must be removed from the run's environment"
        );
    }
}

/// Nothing re-adds a trace target: a run traces nothing of its own either.
#[test]
fn no_trace_target_is_set_in_place_of_the_callers() {
    let mut command = Command::new("runner");
    scrub_runner_git_environment(&mut command);
    for var in CALLER_TRACE_VARS {
        assert_eq!(
            env_of(&command, var),
            Some(None),
            "{var} must be removed, never set to a target of the run's own"
        );
    }
}

/// Routing variables would point a step's git at a repository other than the
/// named checkout. The scrub is hygiene for any child, not a private-copy aid.
#[test]
fn inherited_git_routing_variables_do_not_reach_the_runner_child() {
    let mut command = Command::new("runner");
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env(var, "/caller/chosen/target");
    }
    scrub_runner_git_environment(&mut command);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        assert_eq!(
            env_of(&command, var),
            Some(None),
            "{var} must be removed from the run's environment"
        );
    }
}

/// The scrub must not remove the user's own identity or credentials.
#[test]
fn user_identity_and_credentials_are_left_inherited() {
    let mut command = Command::new("runner");
    command
        .env("GIT_AUTHOR_NAME", "Someone")
        .env("GH_TOKEN", "token");
    scrub_runner_git_environment(&mut command);
    assert_eq!(
        env_of(&command, "GIT_AUTHOR_NAME"),
        Some(Some(OsString::from("Someone")))
    );
    assert_eq!(
        env_of(&command, "GH_TOKEN"),
        Some(Some(OsString::from("token")))
    );
}
