//! Issue #2279 (C9, D5, R8, AC8): `amplihack claude` passes the hooks block
//! as its own settings source, `--settings <AMPLIHACK_HOME>/runtime/
//! launch-hooks-settings.json`, so a stripped `~/.claude/settings.json`
//! cannot leave a session without amplihack's hooks.
//!
//! API this file pins (in `launch::hook_settings` unless noted):
//! - `LAUNCH_FILE = "runtime/launch-hooks-settings.json"` (relative to AMPLIHACK_HOME)
//! - `enum Inject { Path(PathBuf), NotPrivate, Skip(&'static str) }`
//! - `struct Inputs<'a> { binary_name, extra_args, proxy, newer_stamp,
//!    target: Option<&HookTarget>, amplihack_home: &Path }`
//! - `prepare_with(&Inputs) -> Inject` (pure apart from the file write);
//!   production `prepare(binary_name, extra_args)` fills `Inputs` from the
//!   environment.
//! - `user_passed_settings(extra_args) -> bool`
//! - `command::build_command_for_dir(.., contract_file, hook_settings: Option<&Path>)`
//!
//! Claude is never run: the tests read the built `Command` and the file.

use super::command::build_command_for_dir;
use super::hook_settings::{Inject, Inputs, LAUNCH_FILE, prepare_with, user_passed_settings};
use crate::binary_finder::BinaryInfo;
use crate::commands::install::hook_block::{self, HookTarget};
use crate::test_support::home_env_lock;
use serde_json::Value;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn target() -> HookTarget {
    HookTarget {
        bin: PathBuf::from("/opt/amplihack-test/bin/amplihack-hooks"),
        limit_resume: true,
    }
}

fn private_home() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn inputs<'a>(home: &'a Path, extra: &'a [String], t: &'a HookTarget) -> Inputs<'a> {
    Inputs {
        binary_name: "claude",
        extra_args: extra,
        proxy: false,
        newer_stamp: false,
        target: Some(t),
        amplihack_home: home,
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

// ── prepare ───────────────────────────────────────────────────────────────

#[test]
fn prepare_writes_a_hooks_only_file_at_0600_in_a_0700_runtime_dir() {
    let home = private_home();
    let t = target();
    let extra = args(&[]);
    let Inject::Path(path) = prepare_with(&inputs(home.path(), &extra, &t)) else {
        panic!("expected a path");
    };
    assert_eq!(path, home.path().join(LAUNCH_FILE));
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    let v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, vec!["hooks"], "only hooks: no env, no other key");
    assert_eq!(v["hooks"], hook_block::block(&t));
}

#[test]
fn prepare_rewrites_only_when_the_bytes_differ() {
    let home = private_home();
    let t = target();
    let extra = args(&[]);
    let Inject::Path(path) = prepare_with(&inputs(home.path(), &extra, &t)) else {
        panic!("expected a path");
    };
    let ino = std::os::unix::fs::MetadataExt::ino(&fs::metadata(&path).unwrap());
    prepare_with(&inputs(home.path(), &extra, &t));
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&fs::metadata(&path).unwrap()),
        ino,
        "an unchanged file is not replaced"
    );
    let other = HookTarget {
        bin: PathBuf::from("/opt/amplihack-test/bin2/amplihack-hooks"),
        limit_resume: true,
    };
    prepare_with(&inputs(home.path(), &extra, &other));
    let v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(v["hooks"], hook_block::block(&other));
}

#[test]
fn not_claude_is_skipped() {
    let home = private_home();
    let t = target();
    let extra = args(&[]);
    for name in ["copilot", "codex", "amplifier"] {
        let mut i = inputs(home.path(), &extra, &t);
        i.binary_name = name;
        assert!(matches!(prepare_with(&i), Inject::Skip(_)), "{name}");
    }
    assert!(!home.path().join(LAUNCH_FILE).exists());
}

#[test]
fn the_litellm_proxy_is_skipped() {
    let home = private_home();
    let t = target();
    let extra = args(&[]);
    let mut i = inputs(home.path(), &extra, &t);
    i.proxy = true;
    assert!(matches!(prepare_with(&i), Inject::Skip(_)));
    assert!(!home.path().join(LAUNCH_FILE).exists());
}

#[test]
fn a_user_settings_flag_is_skipped_in_both_forms() {
    let home = private_home();
    let t = target();
    for extra in [
        args(&["--settings", "/opt/mine.json"]),
        args(&["--model", "x", "--settings=/opt/mine.json"]),
    ] {
        assert!(user_passed_settings(&extra), "{extra:?}");
        assert!(matches!(
            prepare_with(&inputs(home.path(), &extra, &t)),
            Inject::Skip(_)
        ));
    }
    assert!(!home.path().join(LAUNCH_FILE).exists());
}

#[test]
fn a_settings_flag_after_a_literal_double_dash_does_not_suppress_injection() {
    let home = private_home();
    let t = target();
    let extra = args(&["--", "--settings", "/opt/mine.json"]);
    assert!(!user_passed_settings(&extra));
    assert!(matches!(
        prepare_with(&inputs(home.path(), &extra, &t)),
        Inject::Path(_)
    ));
}

#[test]
fn a_newer_install_stamp_is_skipped_as_an_older_binary() {
    let home = private_home();
    let t = target();
    let extra = args(&[]);
    let mut i = inputs(home.path(), &extra, &t);
    i.newer_stamp = true;
    let Inject::Skip(reason) = prepare_with(&i) else {
        panic!("expected Skip");
    };
    assert!(reason.contains("older binary"), "{reason}");
    assert!(!home.path().join(LAUNCH_FILE).exists());
}

#[test]
fn no_hooks_binary_is_skipped() {
    let home = private_home();
    let extra = args(&[]);
    let t = target();
    let mut i = inputs(home.path(), &extra, &t);
    i.target = None;
    assert!(matches!(prepare_with(&i), Inject::Skip(_)));
}

#[test]
fn a_group_writable_amplihack_home_is_skipped() {
    let home = private_home();
    fs::set_permissions(home.path(), fs::Permissions::from_mode(0o770)).unwrap();
    let t = target();
    let extra = args(&[]);
    assert_eq!(
        prepare_with(&inputs(home.path(), &extra, &t)),
        Inject::NotPrivate
    );
    assert!(!home.path().join(LAUNCH_FILE).exists());
}

#[test]
fn a_symlinked_runtime_dir_is_skipped_and_its_target_untouched() {
    let home = private_home();
    let elsewhere = private_home();
    symlink(elsewhere.path(), home.path().join("runtime")).unwrap();
    let t = target();
    let extra = args(&[]);
    assert_eq!(
        prepare_with(&inputs(home.path(), &extra, &t)),
        Inject::NotPrivate
    );
    assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
}

// ── the built Command ─────────────────────────────────────────────────────

fn claude() -> BinaryInfo {
    BinaryInfo {
        name: "claude".to_string(),
        path: PathBuf::from("/opt/amplihack-test/bin/claude"),
        version: Some("2.1.322".to_string()),
    }
}

fn argv(cmd: &std::process::Command) -> Vec<String> {
    cmd.get_args()
        .map(|a: &OsStr| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn the_command_carries_settings_path_before_the_users_arguments() {
    let _guard = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let settings = PathBuf::from("/opt/amplihack-test/home/runtime/launch-hooks-settings.json");
    let contract = PathBuf::from("/opt/amplihack-test/home/private/system-prompt-append.md");
    let extra = args(&["--print", "hello"]);
    let cmd = build_command_for_dir(
        &claude(),
        false,
        false,
        false,
        &extra,
        None,
        false,
        None,
        Some(&contract),
        Some(&settings),
    )
    .unwrap();
    let a = argv(&cmd);
    let at = a
        .iter()
        .position(|x| x == "--settings")
        .unwrap_or_else(|| panic!("--settings missing: {a:?}"));
    assert_eq!(a[at + 1], settings.to_string_lossy());
    let user = a.iter().position(|x| x == "--print").unwrap();
    assert!(
        at < user,
        "--settings goes before the user's arguments: {a:?}"
    );
    assert!(
        !a.iter().any(|x| x.contains("\"hooks\"")),
        "a path, never inline JSON (#2030): {a:?}"
    );
}

#[test]
fn no_settings_path_means_no_settings_flag() {
    let _guard = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let contract = PathBuf::from("/opt/amplihack-test/home/private/system-prompt-append.md");
    let extra = args(&[]);
    let cmd = build_command_for_dir(
        &claude(),
        false,
        false,
        false,
        &extra,
        None,
        false,
        None,
        Some(&contract),
        None,
    )
    .unwrap();
    assert!(!argv(&cmd).iter().any(|x| x == "--settings"));
}
