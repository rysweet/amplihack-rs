//! Issue #1658 (F): drift guards between the CLI's auto-switch rules and the
//! read-only copies in `amplihack_utils::rotator_status` that the
//! limit-resume hook uses (the hooks crate cannot depend on the CLI).
//!
//! If one of these fails, change both sides together. When #1657's
//! `claude_capacity` lands, collapse the copies onto it instead.

use super::config;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::root_config;
use crate::commands::profile::store::ROOT_ENV;
use amplihack_utils::rotator_status::{auto_switch_on_from, resolve_root_from, valid_live_name};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

fn private_file(p: &Path, bytes: &[u8]) {
    fs::write(p, bytes).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

// ── Toggle ──────────────────────────────────────────────────────────────────

const ENVS: &[Option<&str>] = &[
    None,
    Some(""),
    Some("1"),
    Some("0"),
    Some("yes"),
    Some("true"),
];

const FILES: &[Option<&[u8]>] = &[
    None,
    Some(b"enabled=on\n"),
    Some(b"enabled=off\n"),
    Some(b"enabled=on"),
    Some(b"enabled=on\nthreshold=85\n"),
    Some(b"enabled=on\nthreshold=100\nexclude=work,spare\n"),
    Some(b"enabled=on\nexclude=\n"),
    Some(b"enabled=on\nthreshold=0\n"),
    Some(b"enabled=on\nthreshold=101\n"),
    Some(b"enabled=on\nthreshold=+5\n"),
    Some(b"enabled=on\nexclude=../x\n"),
    Some(b"enabled=on\nexclude=a,,b\n"),
    Some(b"enabled=ON\n"),
    Some(b"enabled =on\n"),
    Some(b"enabled=on\n\n"),
    Some(b"enabled=on\r\n"),
    Some(b"enabled=on\nenabled=off\n"),
    Some(b"threshold=90\n"),
    Some(b"enabled=on\nfuture=1\n"),
    Some(b""),
    Some(b"\n"),
    Some(b"\xff\xfe"),
];

fn cli_on(env: Option<&str>, root: &Path) -> bool {
    config::effective_from(config::env_toggle(env.map(OsStr::new)), &config::read(root)).on
}

#[test]
fn toggle_rule_agrees_on_every_fixture() {
    for file in FILES {
        let root = tempfile::tempdir().unwrap();
        if let Some(bytes) = file {
            private_file(&root.path().join(config::FILE), bytes);
        }
        for env in ENVS {
            assert_eq!(
                auto_switch_on_from(env.map(OsStr::new), root.path()),
                cli_on(*env, root.path()),
                "env={env:?} file={:?}",
                file.map(String::from_utf8_lossy)
            );
        }
    }
}

#[test]
fn toggle_rule_agrees_on_oversize_symlinked_and_foreign_files() {
    let root = tempfile::tempdir().unwrap();
    let mut big = b"enabled=on\n".to_vec();
    big.resize(4097, b'#');
    private_file(&root.path().join(config::FILE), &big);
    assert_eq!(
        auto_switch_on_from(None, root.path()),
        cli_on(None, root.path())
    );

    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    private_file(&real, b"enabled=on\n");
    symlink(&real, root.path().join(config::FILE)).unwrap();
    assert_eq!(
        auto_switch_on_from(None, root.path()),
        cli_on(None, root.path())
    );
    assert!(!cli_on(None, root.path()));

    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(config::FILE)).unwrap();
    assert_eq!(
        auto_switch_on_from(None, root.path()),
        cli_on(None, root.path())
    );
}

// ── Root resolution ─────────────────────────────────────────────────────────

fn env_fn(v: Option<&str>) -> impl Fn(&str) -> Option<OsString> + '_ {
    move |k| (k == ROOT_ENV).then(|| v.map(OsString::from)).flatten()
}

#[test]
fn root_resolution_agrees_with_root_config() {
    let pointers: &[Option<&[u8]>] = &[
        None,
        Some(b"/saved/root\n"),
        Some(b"  /saved/root  \n"),
        Some(b"/saved/root"),
        Some(b"relative\n"),
        Some(b"/a/../b\n"),
        Some(b"\n"),
        Some(b"/tab\troot\n"),
        Some(b"\xff\n"),
    ];
    let envs = [
        None,
        Some(""),
        Some("/env/root"),
        Some("rel"),
        Some("/x/../y"),
    ];
    for ptr in pointers {
        let home = tempfile::tempdir().unwrap();
        let dir = root_config::config_dir(home.path());
        fs::create_dir(&dir).unwrap();
        if let Some(bytes) = ptr {
            private_file(&dir.join(root_config::PROFILE_ROOT_FILE), bytes);
        }
        for env in envs {
            let cli = root_config::resolve_with(env_fn(env), Some(home.path()))
                .ok()
                .map(|(p, _)| p);
            let utils = resolve_root_from(env_fn(env), Some(home.path()));
            assert_eq!(
                utils,
                cli,
                "env={env:?} pointer={:?}",
                ptr.map(String::from_utf8_lossy)
            );
        }
    }
}

#[test]
fn root_resolution_agrees_on_unsafe_pointer_files() {
    // Oversize, symlinked, group-writable.
    let home = tempfile::tempdir().unwrap();
    let dir = root_config::config_dir(home.path());
    fs::create_dir(&dir).unwrap();
    let ptr = dir.join(root_config::PROFILE_ROOT_FILE);

    private_file(&ptr, &[b'/'; 4097]);
    let cli = root_config::resolve_with(env_fn(None), Some(home.path()))
        .ok()
        .map(|(p, _)| p);
    assert_eq!(resolve_root_from(env_fn(None), Some(home.path())), cli);

    fs::remove_file(&ptr).unwrap();
    let real = home.path().join("real");
    private_file(&real, b"/saved/root\n");
    symlink(&real, &ptr).unwrap();
    let cli = root_config::resolve_with(env_fn(None), Some(home.path()))
        .ok()
        .map(|(p, _)| p);
    assert_eq!(cli, None);
    assert_eq!(resolve_root_from(env_fn(None), Some(home.path())), cli);

    fs::remove_file(&ptr).unwrap();
    private_file(&ptr, b"/saved/root\n");
    fs::set_permissions(&ptr, fs::Permissions::from_mode(0o620)).unwrap();
    let cli = root_config::resolve_with(env_fn(None), Some(home.path()))
        .ok()
        .map(|(p, _)| p);
    assert_eq!(cli, None);
    assert_eq!(resolve_root_from(env_fn(None), Some(home.path())), cli);
}

// ── Live name rule ──────────────────────────────────────────────────────────

#[test]
fn live_name_rule_agrees_with_profile_name() {
    let long_ok = "a".repeat(64);
    let long_bad = "a".repeat(65);
    for n in [
        "work",
        "spare-2",
        "a.b_c",
        "A1",
        "1",
        long_ok.as_str(),
        long_bad.as_str(),
        "",
        "-x",
        "_x",
        ".x",
        "a..b",
        "a.",
        "a/b",
        "a\\b",
        "a b",
        "a\nb",
        "CON",
        "con.txt",
        "COM1",
        "com0",
        "LPT9",
        "CONIN$",
        "é",
        "aé",
    ] {
        assert_eq!(
            valid_live_name(n),
            ProfileName::parse(n).is_ok(),
            "name {n:?}"
        );
    }
}

// ── fsx is a re-export of amplihack_utils::secure_files ─────────────────────

#[test]
fn fsx_re_exports_the_moved_helpers_instead_of_defining_them() {
    let src = include_str!("fsx.rs");
    assert!(
        src.contains("amplihack_utils::secure_files"),
        "fsx.rs must re-export from amplihack_utils::secure_files"
    );
    for name in [
        "read_bounded",
        "read_bounded_nofollow",
        "write_private_atomic",
        "ensure_private_dir",
        "check_private_dir",
        "writable_by_others",
        "euid",
    ] {
        assert!(
            !src.contains(&format!("fn {name}(")) && !src.contains(&format!("fn {name}<")),
            "fsx.rs still defines `{name}`; it must re-export the utils one"
        );
        assert!(src.contains(name), "fsx.rs must still export `{name}`");
    }
}
