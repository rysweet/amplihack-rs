//! `wait_config::write` (issue #1618): replaces only the `usage_limit_wait`
//! key of `$HOME/.amplihack/config`, keeps every other key, writes
//! atomically at mode 0600, and refuses a symlinked file or content that is
//! not a JSON object, or a `~/.amplihack` that others can write. A file or
//! directory owned by another user cannot be made without root, so that
//! refusal is not tested here.
//!
//! Wired from `lib.rs` with `#[cfg(all(test, unix))] mod tests_wait_config_write;`.

use crate::wait_config::{WaitSettings, read_file, resolve_from, write};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn defaults() -> WaitSettings {
    WaitSettings {
        enabled: true,
        max_wait_secs: None,
    }
}

fn home() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".amplihack")).unwrap();
    (tmp, home)
}

fn config(home: &Path) -> PathBuf {
    home.join(".amplihack").join("config")
}

fn put(home: &Path, body: &str, mode: u32) {
    let p = config(home);
    fs::write(&p, body).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
}

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

fn read_json(home: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(config(home)).unwrap()).unwrap()
}

#[test]
fn write_replaces_only_its_key_and_keeps_every_other_key() {
    let (_t, home) = home();
    put(
        &home,
        r#"{"node_options_consent":true,"node_options_limit_mb":8192,"usage_limit_wait":{"enabled":true},"other":{"x":[1,2]}}"#,
        0o600,
    );
    let s = WaitSettings {
        enabled: false,
        max_wait_secs: Some(3600),
    };
    write(&home, &s).unwrap();
    let v = read_json(&home);
    assert_eq!(v["node_options_consent"], true);
    assert_eq!(v["node_options_limit_mb"], 8192);
    assert_eq!(v["other"]["x"], serde_json::json!([1, 2]));
    assert_eq!(v["usage_limit_wait"]["enabled"], false);
    assert_eq!(v["usage_limit_wait"]["max_wait_secs"], 3600);
    assert_eq!(mode(&config(&home)), 0o600);
}

#[test]
fn write_creates_a_private_file_and_round_trips() {
    let (_t, home) = home();
    for s in [
        defaults(),
        WaitSettings {
            enabled: false,
            max_wait_secs: Some(0),
        },
        WaitSettings {
            enabled: true,
            max_wait_secs: Some(u64::MAX),
        },
    ] {
        write(&home, &s).unwrap();
        assert_eq!(mode(&config(&home)), 0o600);
        let (got, warns) = resolve_from(None, None, read_file(&home));
        assert_eq!(got, s);
        assert!(warns.is_empty(), "{warns:?}");
    }
    // No temporary file is left beside it.
    let names: Vec<_> = fs::read_dir(home.join(".amplihack"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("config")]);
}

#[test]
fn write_tightens_a_0644_file_to_0600() {
    let (_t, home) = home();
    put(&home, r#"{"node_options_consent":false}"#, 0o644);
    write(&home, &defaults()).unwrap();
    assert_eq!(mode(&config(&home)), 0o600);
    assert_eq!(read_json(&home)["node_options_consent"], false);
}

#[test]
fn write_refuses_a_symlinked_file_and_leaves_both_untouched() {
    let (tmp, home) = home();
    let target = tmp.path().join("target.json");
    fs::write(&target, r#"{"keep":1}"#).unwrap();
    symlink(&target, config(&home)).unwrap();
    assert!(write(&home, &defaults()).is_err());
    assert!(
        fs::symlink_metadata(config(&home))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), r#"{"keep":1}"#);
}

#[test]
fn write_refuses_content_that_is_not_a_json_object() {
    let (_t, home) = home();
    for body in ["[1,2]", "not json", "\"str\"", "null"] {
        put(&home, body, 0o600);
        assert!(write(&home, &defaults()).is_err(), "{body}");
        assert_eq!(fs::read_to_string(config(&home)).unwrap(), body);
    }
}

#[test]
fn write_refuses_an_amplihack_dir_others_can_write_and_leaves_it_alone() {
    let (_t, home) = home();
    let dir = home.join(".amplihack");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(write(&home, &defaults()).is_err());
    assert!(!config(&home).exists(), "nothing written");
    assert_eq!(mode(&dir), 0o777, "never chmod-ed");
}

#[test]
fn write_creates_a_missing_amplihack_dir_private() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    write(&home, &defaults()).unwrap();
    assert_eq!(mode(&home.join(".amplihack")), 0o700);
    assert_eq!(mode(&config(&home)), 0o600);
}
