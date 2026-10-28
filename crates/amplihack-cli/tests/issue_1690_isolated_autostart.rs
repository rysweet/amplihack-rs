//! Issue #1690: a test that starts the `amplihack` binary through
//! `isolated(..)` must never autostart the real Claude login rotator.
//!
//! `isolated(..)` sets `AMPLIHACK_ROTATE_AUTOSTART=0` (exposed as
//! `ROTATE_AUTOSTART_ENV`) next to `AMPLIHACK_TEST_HOME_GUARD=1`. The binary's
//! own check on the guard is the stop that holds when a fixture clears the
//! environment afterwards; this one covers every child that keeps it.
//!
//! These read the built `Command` only; nothing is spawned.

use crate::isolated_amplihack::{HOME_GUARD_ENV, ROTATE_AUTOSTART_ENV, isolated};
use std::ffi::OsStr;
use std::process::Command;

fn env_of<'a>(cmd: &'a Command, key: &str) -> Option<Option<&'a OsStr>> {
    cmd.get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v)
}

#[test]
fn isolated_turns_rotator_autostart_off() {
    let cmd = isolated("x");
    assert_eq!(
        env_of(&cmd, ROTATE_AUTOSTART_ENV),
        Some(Some(OsStr::new("0"))),
        "isolated() must set {ROTATE_AUTOSTART_ENV}=0"
    );
}

#[test]
fn isolated_still_sets_the_home_guard() {
    let cmd = isolated("x");
    assert_eq!(
        env_of(&cmd, HOME_GUARD_ENV),
        Some(Some(OsStr::new("1"))),
        "the binary's autostart skip keys off this"
    );
}

#[test]
fn isolated_never_uses_the_real_home() {
    let cmd = isolated("x");
    let home = env_of(&cmd, "HOME")
        .flatten()
        .expect("isolated() sets HOME");
    if let Some(real) = std::env::var_os("HOME") {
        assert_ne!(home, real.as_os_str(), "HOME must be a temp dir");
    }
    assert!(
        std::path::Path::new(home).starts_with(std::env::temp_dir()),
        "HOME {home:?} is under the temp dir"
    );
}
