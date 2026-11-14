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

/// Issue #2040: the running-session snapshot lives in `$XDG_RUNTIME_DIR`, so
/// a child started by a test must get a private runtime directory under the
/// temp dir, never the developer's real one.
#[test]
fn isolated_sets_a_private_runtime_dir_under_the_temp_dir() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let cmd = isolated("x");
    let rt = env_of(&cmd, "XDG_RUNTIME_DIR")
        .flatten()
        .expect("isolated() sets XDG_RUNTIME_DIR");
    if let Some(real) = std::env::var_os("XDG_RUNTIME_DIR") {
        assert_ne!(rt, real.as_os_str(), "XDG_RUNTIME_DIR must be a temp dir");
    }
    let rt = std::path::Path::new(rt);
    assert!(rt.is_absolute(), "{rt:?}");
    assert!(
        rt.starts_with(std::env::temp_dir()),
        "XDG_RUNTIME_DIR {rt:?} is under the temp dir"
    );
    let meta = std::fs::symlink_metadata(rt).expect("the runtime dir exists");
    assert!(meta.is_dir(), "{rt:?} is a directory, not a symlink");
    assert_eq!(meta.permissions().mode() & 0o777, 0o700, "{rt:?}");
    assert_eq!(meta.uid(), amplihack_utils::secure_files::euid());
}
