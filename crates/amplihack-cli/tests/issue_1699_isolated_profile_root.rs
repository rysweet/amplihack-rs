//! Issue #1699: a test that starts the `amplihack` binary through
//! `isolated(..)` must not pass on `AMPLIHACK_CLAUDE_PROFILE_ROOT`, the
//! variable that let `amplihack install` under a temp HOME find and rewrite
//! the developer's real profiles.
//!
//! `isolated(..)` already calls `env_clear()`, and also removes the variable
//! by name. After `env_clear()`, std drops a removed key from `get_envs()`
//! instead of listing it as `(key, None)`; without the clear, the removal
//! shows as `(key, None)`. Either way the child never receives a value, which
//! is what these tests assert.
//!
//! These read the built `Command` only; nothing is spawned and the test
//! process environment is never changed.

use crate::isolated_amplihack::{PROFILE_ROOT_ENV, isolated};
use std::ffi::OsStr;
use std::process::Command;

fn env_of<'a>(cmd: &'a Command, key: &str) -> Option<Option<&'a OsStr>> {
    cmd.get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v)
}

#[test]
fn the_constant_names_the_profile_root_variable() {
    assert_eq!(PROFILE_ROOT_ENV, "AMPLIHACK_CLAUDE_PROFILE_ROOT");
}

#[test]
fn isolated_never_passes_the_profile_root_variable_on() {
    let cmd = isolated("x");
    let value = env_of(&cmd, PROFILE_ROOT_ENV);
    assert!(
        !matches!(value, Some(Some(_))),
        "isolated() must not give the child {PROFILE_ROOT_ENV}: {value:?}"
    );
}

#[test]
fn the_explicit_removal_holds_even_without_env_clear() {
    // What isolated() does after env_clear(), on a Command that inherits.
    let mut cmd = Command::new("x");
    cmd.env(PROFILE_ROOT_ENV, "/somewhere/real")
        .env_remove(PROFILE_ROOT_ENV);
    assert_eq!(env_of(&cmd, PROFILE_ROOT_ENV), Some(None));
}

#[test]
fn a_test_can_still_set_its_own_profile_root_afterwards() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("profiles");
    let mut cmd = isolated("x");
    cmd.env(PROFILE_ROOT_ENV, &root);
    assert_eq!(env_of(&cmd, PROFILE_ROOT_ENV), Some(Some(root.as_os_str())));
}
