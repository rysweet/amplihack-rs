use super::command::build_command_for_dir;
use crate::binary_finder::BinaryInfo;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

fn bin(name: &str) -> BinaryInfo {
    BinaryInfo {
        name: name.into(),
        path: PathBuf::from("/nonexistent/bin"),
        version: None,
    }
}

fn cfg_env(cmd: &std::process::Command) -> Option<Option<std::ffi::OsString>> {
    cmd.get_envs()
        .find(|(k, _)| *k == OsStr::new("CLAUDE_CONFIG_DIR"))
        .map(|(_, v)| v.map(|s| s.to_os_string()))
}

fn build(name: &str, dir: Option<&Path>) -> std::process::Command {
    build_command_for_dir(&bin(name), false, false, false, &[], None, false, dir)
}

#[test]
fn profile_dir_is_set_for_claude_compatible_binaries() {
    let dir = Path::new("/tmp/profiles/work");
    for name in ["claude", "rusty", "rustyclawd", "amplifier"] {
        let cmd = build(name, Some(dir));
        assert_eq!(
            cfg_env(&cmd),
            Some(Some(dir.as_os_str().to_os_string())),
            "{name}"
        );
    }
}

#[test]
fn no_profile_leaves_env_untouched() {
    let cmd = build("claude", None);
    assert_eq!(
        cfg_env(&cmd),
        None,
        "must neither set nor remove the variable"
    );
}

#[test]
fn copilot_and_codex_are_unaffected() {
    let dir = Path::new("/tmp/profiles/work");
    for name in ["copilot", "codex"] {
        assert_eq!(cfg_env(&build(name, Some(dir))), None, "{name}");
    }
}

#[test]
fn profile_overrides_ambient_in_child_without_mutating_parent() {
    let _g = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let prev = std::env::var_os("CLAUDE_CONFIG_DIR");
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", "/ambient") };
    let cmd = build("claude", Some(Path::new("/tmp/profiles/work")));
    let child = cfg_env(&cmd).flatten().unwrap();
    let parent = std::env::var_os("CLAUDE_CONFIG_DIR");
    unsafe {
        match prev {
            Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
            None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
        }
    }
    assert_eq!(child, OsStr::new("/tmp/profiles/work"));
    assert_eq!(parent.as_deref(), Some(OsStr::new("/ambient")));
}

#[test]
fn ambient_passes_through_when_no_profile() {
    let cmd = build("claude", None);
    assert!(cfg_env(&cmd).is_none());
}

// ---------------------------------------------------------------------------
// Issue #1567: the resolved profile is pinned for every descendant launch.
// ---------------------------------------------------------------------------

const PROFILE_VAR: &str = "AMPLIHACK_CLAUDE_PROFILE";
const ROOT_VAR: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";

fn child_env(cmd: &std::process::Command, key: &str) -> Option<Option<std::ffi::OsString>> {
    cmd.get_envs()
        .find(|(k, _)| *k == OsStr::new(key))
        .map(|(_, v)| v.map(|s| s.to_os_string()))
}

/// Runs `f` with `ROOT_VAR` set to `value` (or removed), restoring it after.
fn with_root_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
    crate::test_support::with_env(&[(ROOT_VAR, value.map(OsStr::new))], f)
}

#[test]
fn profile_name_is_pinned_alongside_config_dir() {
    let dir = Path::new("/tmp/profiles/work");
    for name in ["claude", "rusty", "rustyclawd", "amplifier"] {
        let cmd = build(name, Some(dir));
        assert!(cfg_env(&cmd).flatten().is_some(), "{name}");
        assert_eq!(
            child_env(&cmd, PROFILE_VAR),
            Some(Some("work".into())),
            "{name}: AMPLIHACK_CLAUDE_PROFILE must equal the dir's file name"
        );
    }
}

#[test]
fn profile_root_is_passed_only_when_parent_had_root_env() {
    let dir = Path::new("/tmp/profiles/work");
    let without = with_root_env(None, || build("claude", Some(dir)));
    assert_eq!(child_env(&without, ROOT_VAR), None);
    assert_eq!(child_env(&without, PROFILE_VAR), Some(Some("work".into())));

    let with = with_root_env(Some("/tmp/profiles"), || build("claude", Some(dir)));
    assert_eq!(
        child_env(&with, ROOT_VAR),
        Some(Some("/tmp/profiles".into())),
        "the root passed down is the parent of the resolved dir"
    );
}

#[test]
fn copilot_and_codex_get_no_profile_pin() {
    let dir = Path::new("/tmp/profiles/work");
    with_root_env(Some("/tmp/profiles"), || {
        for name in ["copilot", "codex"] {
            let cmd = build(name, Some(dir));
            assert_eq!(child_env(&cmd, PROFILE_VAR), None, "{name}");
            assert_eq!(child_env(&cmd, ROOT_VAR), None, "{name}");
        }
    });
}

#[test]
fn no_profile_sets_no_profile_pin() {
    let cmd = with_root_env(Some("/tmp/profiles"), || build("claude", None));
    assert_eq!(child_env(&cmd, PROFILE_VAR), None);
    assert_eq!(child_env(&cmd, ROOT_VAR), None);
}

#[test]
fn profile_pin_is_child_only() {
    let _g = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let prev = std::env::var_os(PROFILE_VAR);
    let _cmd = build("claude", Some(Path::new("/tmp/profiles/work")));
    assert_eq!(
        std::env::var_os(PROFILE_VAR),
        prev,
        "building the child must not change the parent environment"
    );
}
