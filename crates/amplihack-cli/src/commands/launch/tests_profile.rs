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
