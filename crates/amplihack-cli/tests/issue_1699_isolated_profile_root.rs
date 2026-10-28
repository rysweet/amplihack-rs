//! Issue #1699: a child started through `isolated(..)` must never see the
//! developer's `AMPLIHACK_CLAUDE_PROFILE_ROOT`, or an `amplihack install` under
//! a temp HOME would refresh the real profiles' settings.json.
//!
//! `env_clear()` is the main control; `isolated(..)` also removes
//! `PROFILE_ROOT_ENV` explicitly after copying the kept variables, so a future
//! change to the kept list cannot let it through. After `env_clear()`, std's
//! `env_remove` deletes the key rather than recording `(key, None)`, so the
//! test asserts the observable result: the built `Command` gives the child no
//! value for the variable. These read the built `Command` only; nothing is
//! spawned.

use crate::isolated_amplihack::{PROFILE_ROOT_ENV, isolated};
use crate::support::quiet_process::{child_set_var, in_quiet_process};
use std::ffi::OsStr;
use std::process::Command;

fn env_of<'a>(cmd: &'a Command, key: &str) -> Option<Option<&'a OsStr>> {
    cmd.get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v)
}

#[test]
fn isolated_never_passes_the_profile_root() {
    in_quiet_process(
        module_path!(),
        "isolated_never_passes_the_profile_root",
        || {
            child_set_var(PROFILE_ROOT_ENV, "/home/someone/.claude-profiles");
            let cmd = isolated("x");
            assert!(
                matches!(env_of(&cmd, PROFILE_ROOT_ENV), None | Some(None)),
                "isolated() must never pass {PROFILE_ROOT_ENV} to the child"
            );
        },
    );
}

#[test]
fn profile_root_env_name_matches_the_binary() {
    assert_eq!(PROFILE_ROOT_ENV, "AMPLIHACK_CLAUDE_PROFILE_ROOT");
    let source = include_str!("../../amplihack-utils/src/rotator_status/root.rs");
    assert!(
        source.contains(&format!("\"{PROFILE_ROOT_ENV}\"")),
        "the binary reads its profile root from {PROFILE_ROOT_ENV}"
    );
}
