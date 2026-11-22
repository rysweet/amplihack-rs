//! Issue #2922 (TDD): the contract of the shared profile-isolation helper.
//!
//! The helper lives in `tests/support/claude_profile_isolation.rs` and is
//! included into `crate::test_support`. These tests fail until it exists.
//!
//! Every value here is synthetic and sits in a temporary folder.

use crate::test_support::claude_profile_isolation::{
    CLAUDE_PROFILE_ENV_VARS, ClaudeProfileIsolation, purge_inherited, remove_from_command,
    with_config_dir,
};
use crate::test_support::{EnvGuard, HomeGuard, env_lock, tempdir_outside_framework_roots};
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

fn lock() -> std::sync::MutexGuard<'static, ()> {
    env_lock().lock().unwrap_or_else(|e| e.into_inner())
}

fn get(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

fn export_all(value: &Path) -> EnvGuard {
    let value = value.to_str().unwrap();
    let mut guard = EnvGuard::capture(&CLAUDE_PROFILE_ENV_VARS);
    for name in CLAUDE_PROFILE_ENV_VARS {
        guard.assign(name, value);
    }
    guard
}

#[test]
fn the_variable_list_is_exactly_the_five_profile_redirecting_names() {
    let mut names = CLAUDE_PROFILE_ENV_VARS.to_vec();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "AMPLIHACK_CLAUDE_AUTO_SWITCH",
            "AMPLIHACK_CLAUDE_PROFILE",
            "AMPLIHACK_CLAUDE_PROFILE_ROOT",
            "AMPLIHACK_HOME",
            "CLAUDE_CONFIG_DIR",
        ]
    );
}

/// A product rename must fail the build here, not leave the helper clearing a
/// name nothing reads.
#[test]
fn the_list_holds_the_names_the_product_exports() {
    use amplihack_utils::rotator_status::ROOT_ENV;
    use amplihack_utils::rotator_status::TOGGLE_ENV;
    for name in [crate::commands::profile::PROFILE_ENV, ROOT_ENV, TOGGLE_ENV] {
        assert!(CLAUDE_PROFILE_ENV_VARS.contains(&name), "{name} is missing");
    }
}

#[test]
fn a_new_guard_clears_all_five_and_drop_restores_them() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let _outer = export_all(scratch.path());
    {
        let _iso = ClaudeProfileIsolation::new();
        for name in CLAUDE_PROFILE_ENV_VARS {
            assert_eq!(get(name), None, "{name} must be cleared");
        }
    }
    for name in CLAUDE_PROFILE_ENV_VARS {
        assert_eq!(
            get(name).as_deref(),
            Some(scratch.path().as_os_str()),
            "{name} must be restored"
        );
    }
}

#[test]
fn drop_restores_the_prior_values_when_the_test_panics() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let _outer = export_all(scratch.path());
    let result = std::panic::catch_unwind(|| {
        let _iso = ClaudeProfileIsolation::new();
        panic!("simulated test failure");
    });
    assert!(result.is_err());
    for name in CLAUDE_PROFILE_ENV_VARS {
        assert_eq!(
            get(name).as_deref(),
            Some(scratch.path().as_os_str()),
            "{name} leaked a cleared state to sibling tests"
        );
    }
}

#[test]
fn a_variable_that_was_unset_stays_unset_after_drop() {
    let _lock = lock();
    let _outer = EnvGuard::unset(CLAUDE_PROFILE_ENV_VARS);
    drop(ClaudeProfileIsolation::new());
    for name in CLAUDE_PROFILE_ENV_VARS {
        assert_eq!(get(name), None, "{name}");
    }
}

#[test]
fn purge_is_idempotent_and_leaves_the_names_absent() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let _outer = export_all(scratch.path());
    purge_inherited();
    purge_inherited();
    for name in CLAUDE_PROFILE_ENV_VARS {
        assert_eq!(get(name), None, "{name}");
    }
}

#[test]
fn home_guard_isolates_the_five_while_held_and_restores_them_after() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let home = scratch.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _outer = export_all(scratch.path());
    {
        let _home = HomeGuard::set(&home);
        for name in CLAUDE_PROFILE_ENV_VARS {
            assert_eq!(get(name), None, "{name} must be cleared under HomeGuard");
        }
    }
    for name in CLAUDE_PROFILE_ENV_VARS {
        assert_eq!(
            get(name).as_deref(),
            Some(scratch.path().as_os_str()),
            "{name} must be restored to its prior value"
        );
    }
}

#[test]
fn env_guard_isolates_the_five_when_it_sets_home() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let _outer = export_all(scratch.path());
    {
        let _env = EnvGuard::set([("HOME", scratch.path().to_str().unwrap())]);
        for name in CLAUDE_PROFILE_ENV_VARS {
            assert_eq!(get(name), None, "{name} must be cleared with HOME");
        }
    }
    for name in CLAUDE_PROFILE_ENV_VARS {
        assert_eq!(
            get(name).as_deref(),
            Some(scratch.path().as_os_str()),
            "{name} must be restored to its prior value"
        );
    }
}

/// A guard that set a profile variable and then moved `HOME` must not leave
/// the value it set behind when it drops.
#[test]
fn env_guard_restores_a_profile_variable_it_set_before_moving_home() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let _outer = EnvGuard::unset(CLAUDE_PROFILE_ENV_VARS);
    {
        let mut env = EnvGuard::capture(&[]);
        env.assign("AMPLIHACK_HOME", scratch.path());
        env.assign("HOME", scratch.path());
        assert_eq!(get("AMPLIHACK_HOME"), None, "moving HOME must clear it");
    }
    assert_eq!(
        get("AMPLIHACK_HOME"),
        None,
        "the guard leaked its own value"
    );
}

/// A profile variable named in the same `EnvGuard::set` call as `HOME` keeps
/// the value given, wherever it sits in the array, and is restored on drop.
#[test]
fn env_guard_set_keeps_a_profile_variable_named_with_home() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let value = scratch.path().to_str().unwrap();
    let _outer = EnvGuard::unset(CLAUDE_PROFILE_ENV_VARS);
    {
        let _env = EnvGuard::set([("AMPLIHACK_HOME", value), ("HOME", value)]);
        assert_eq!(
            get("AMPLIHACK_HOME").as_deref(),
            Some(scratch.path().as_os_str()),
            "a profile variable named with HOME must keep its value"
        );
    }
    assert_eq!(
        get("AMPLIHACK_HOME"),
        None,
        "the guard leaked its own value"
    );
}

#[test]
fn env_guard_leaves_the_five_alone_when_home_is_not_among_its_keys() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let _outer = export_all(scratch.path());
    let _env = EnvGuard::set([("AMPLIHACK_ISSUE_2922_UNRELATED", "1")]);
    for name in CLAUDE_PROFILE_ENV_VARS {
        assert!(get(name).is_some(), "{name} must not be touched");
    }
}

/// The child names the variables it holds through `${NAME+NAME}`, which
/// expands to nothing when `NAME` is unset, rather than by matching lines of
/// `env` output. A test binary launched from a recipe step inherits variables
/// whose values are whole agent transcripts, and such a value can hold a line
/// that begins `CLAUDE_CONFIG_DIR=`; grepping `env` reports those lines as if
/// the child held the variable.
#[test]
fn remove_from_command_strips_all_five_from_a_child() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let _outer = export_all(scratch.path());
    let names = CLAUDE_PROFILE_ENV_VARS
        .iter()
        .map(|name| format!("${{{name}+{name}}}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut cmd = Command::new("sh");
    cmd.args(["-c", &format!("printf '%s\\n' {names}")]);
    remove_from_command(&mut cmd);
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");
}

/// The probe above must report a variable that is still set, or it would pass
/// against a `remove_from_command` that did nothing.
#[test]
fn the_child_probe_names_a_variable_that_is_still_set() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let _outer = export_all(scratch.path());
    let names = CLAUDE_PROFILE_ENV_VARS
        .iter()
        .map(|name| format!("${{{name}+{name}}}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut cmd = Command::new("sh");
    cmd.args(["-c", &format!("printf '%s\\n' {names}")]);
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut reported = stdout
        .lines()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    reported.sort_unstable();
    let mut expected = CLAUDE_PROFILE_ENV_VARS.to_vec();
    expected.sort_unstable();
    assert_eq!(reported, expected);
}

// with_config_dir: the only way a test may keep CLAUDE_CONFIG_DIR set, and
// only inside its temporary HOME.

#[test]
fn with_config_dir_accepts_a_nested_path_and_restores_on_drop() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let home = scratch.path().join("home");
    let dir = home.join(".claude-custom");
    std::fs::create_dir_all(&dir).unwrap();
    let _outer = EnvGuard::unset(CLAUDE_PROFILE_ENV_VARS);
    {
        let _iso = with_config_dir(&home, &dir);
        assert_eq!(get("CLAUDE_CONFIG_DIR").as_deref(), Some(dir.as_os_str()));
    }
    assert_eq!(get("CLAUDE_CONFIG_DIR"), None);
}

#[test]
fn with_config_dir_accepts_a_directory_that_does_not_exist_yet() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let home = scratch.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let dir = home.join("later");
    let _outer = EnvGuard::unset(CLAUDE_PROFILE_ENV_VARS);
    let _iso = with_config_dir(&home, &dir);
    assert_eq!(get("CLAUDE_CONFIG_DIR").as_deref(), Some(dir.as_os_str()));
}

#[test]
#[should_panic(expected = "outside")]
fn with_config_dir_panics_for_a_directory_outside_home() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let home = scratch.path().join("home");
    let other = scratch.path().join("other");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let _iso = with_config_dir(&home, &other);
}

#[test]
#[should_panic(expected = "outside")]
fn with_config_dir_panics_for_a_dotdot_escape() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let home = scratch.path().join("home");
    let other = scratch.path().join("other");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let _iso = with_config_dir(&home, &home.join("..").join("other"));
}

#[cfg(unix)]
#[test]
#[should_panic(expected = "outside")]
fn with_config_dir_panics_for_a_symlink_planted_inside_home() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let home = scratch.path().join("home");
    let other = scratch.path().join("other");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let link = home.join("link");
    std::os::unix::fs::symlink(&other, &link).unwrap();
    let _iso = with_config_dir(&home, &link);
}

/// `/x/h` is a string prefix of `/x/home2` but not a path ancestor.
#[test]
#[should_panic(expected = "outside")]
fn with_config_dir_compares_components_not_string_prefixes() {
    let _lock = lock();
    let scratch = tempdir_outside_framework_roots();
    let home = scratch.path().join("h");
    let sibling = scratch.path().join("home2");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    let _iso = with_config_dir(&home, &sibling);
}
