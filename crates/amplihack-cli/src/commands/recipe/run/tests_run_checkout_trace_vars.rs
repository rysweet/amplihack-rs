//! Issue #2738: a caller's Git Trace2 settings do not follow a run.
//!
//! The Git-command tracer removed in #2738 cleared these variables as a side
//! effect of owning its own trace target. `RunCheckout::apply_to` now owns
//! them for their own reason, so this asserts the behaviour directly rather
//! than through whatever happens to clear them.
//!
//! `GIT_TRACE2_REDACT` is the one with teeth: Git redacts `Authorization:`
//! headers in Trace2 output by default, and `=0` turns that off. A caller
//! environment carrying a `GIT_TRACE2*` target together with `=0` would
//! otherwise write this run's auth headers to a sink of the caller's
//! choosing.

use super::run_checkout::{CALLER_TRACE_VARS, RunCheckout};
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

/// A caller checkout is still a run: the clearing is outside the state match.
#[test]
fn a_caller_checkout_clears_the_callers_trace_settings() {
    let mut command = Command::new("runner");
    for var in CALLER_TRACE_VARS {
        command.env(var, "/caller/chosen/target");
    }
    RunCheckout::Caller(std::path::PathBuf::from("/caller")).apply_to(&mut command);
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
    RunCheckout::Caller(std::path::PathBuf::from("/caller")).apply_to(&mut command);
    for var in CALLER_TRACE_VARS {
        assert_eq!(
            env_of(&command, var),
            Some(None),
            "{var} must be removed, never set to a target of the run's own"
        );
    }
}
