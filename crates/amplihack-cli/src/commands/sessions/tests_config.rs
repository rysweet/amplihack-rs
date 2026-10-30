//! `sessions::config` (issue #1746): flag, then user config key, then
//! default.
//!
//! Contract:
//! - `resolve(days: Option<u32>, dest: Option<PathBuf>, apply: bool,
//!   home: Option<PathBuf>) -> Result<Opts, ConfigError>`.
//! - `Opts { home, days, dest, apply }`; `dest` is always absolute.
//! - Keys: `sessions.archive_days` and `sessions.archive_dest` in
//!   `<home>/.amplihack/config.toml`, read on every call. A key is read only
//!   when its flag is absent.
//! - Defaults: 3 days, `<home>/.amplihack/session-archive`.
//! - A file that cannot give a key a value (unparseable, an invalid value,
//!   writable by others) is `ConfigError::Key`, displayed as
//!   `cannot read <key> (<why>)`; never the default, and never the value.
//! - No environment variable is read for days or DEST (checked end to end
//!   in `tests/issue_1746_sessions_archive.rs`).

use super::config::{ConfigError, DEFAULT_DAYS, outside_test_temp, resolve};
use crate::user_config::{Value, schema};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

struct Home {
    _tmp: tempfile::TempDir,
    path: PathBuf,
}

impl Home {
    fn file(&self) -> PathBuf {
        self.path.join(".amplihack/config.toml")
    }

    fn write(&self, body: &str) {
        fs::write(self.file(), body).unwrap();
        fs::set_permissions(self.file(), fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn arg(&self) -> Option<PathBuf> {
        Some(self.path.clone())
    }
}

/// A home under the temp dir, with `body` as its user config file.
fn home(body: Option<&str>) -> Home {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().canonicalize().unwrap().join("home");
    fs::create_dir_all(path.join(".amplihack")).unwrap();
    let h = Home { _tmp: tmp, path };
    if let Some(body) = body {
        h.write(body);
    }
    h
}

#[test]
fn defaults() {
    let h = home(None);
    let o = resolve(None, None, false, h.arg()).unwrap();
    assert_eq!(o.days, 3);
    assert_eq!(o.dest, h.path.join(".amplihack/session-archive"));
    assert_eq!(o.home, h.path);
    assert!(!o.apply);
}

#[test]
fn the_schema_default_is_the_default() {
    let key = schema::key("sessions.archive_days").unwrap();
    assert_eq!(
        key.default_value(),
        Some(Value::Int(u64::from(DEFAULT_DAYS)))
    );
    assert_eq!(schema::key("sessions.archive_dest").unwrap().default, None);
}

#[test]
fn keys_override_the_default_and_flags_override_keys() {
    let h = home(Some(
        "[sessions]\narchive_days = 9\narchive_dest = \"/var/a\"\n",
    ));
    let o = resolve(None, None, true, h.arg()).unwrap();
    assert_eq!(
        (o.days, o.dest.clone(), o.apply),
        (9, PathBuf::from("/var/a"), true)
    );
    let o = resolve(Some(2), Some(PathBuf::from("/var/b")), false, h.arg()).unwrap();
    assert_eq!((o.days, o.dest), (2, PathBuf::from("/var/b")));
}

#[test]
fn every_call_reads_the_file_again() {
    // The rotator calls resolve on each due pass, so this is what makes a
    // running rotator follow `config set`.
    let h = home(Some("[sessions]\narchive_days = 9\n"));
    assert_eq!(resolve(None, None, true, h.arg()).unwrap().days, 9);
    h.write("[sessions]\narchive_days = 5\narchive_dest = \"/var/c\"\n");
    let o = resolve(None, None, true, h.arg()).unwrap();
    assert_eq!((o.days, o.dest), (5, PathBuf::from("/var/c")));
}

#[test]
fn an_invalid_value_is_an_error_that_names_the_key_but_not_the_value() {
    for (body, key) in [
        ("archive_days = 0", "sessions.archive_days"),
        ("archive_days = -1", "sessions.archive_days"),
        ("archive_days = 4294967296", "sessions.archive_days"),
        ("archive_days = \"SECRET-VALUE\"", "sessions.archive_days"),
        (
            "archive_dest = \"SECRET-VALUE/rel\"",
            "sessions.archive_dest",
        ),
        (
            "archive_dest = \"/a/../SECRET-VALUE\"",
            "sessions.archive_dest",
        ),
        ("archive_dest = 7", "sessions.archive_dest"),
    ] {
        let h = home(Some(&format!("[sessions]\n{body}\n")));
        let err = resolve(None, None, false, h.arg()).unwrap_err();
        assert!(
            matches!(&err, ConfigError::Key { key: k, .. } if *k == key),
            "{body}: {err:?}"
        );
        let msg = err.to_string();
        let file = h.file();
        assert!(
            msg.starts_with(&format!("cannot read {key} ({}: {key}: ", file.display())),
            "{body}: {msg}"
        );
        assert!(!msg.contains("SECRET"), "{body}: {msg}");
    }
}

#[test]
fn an_unparseable_file_is_an_error_not_the_default() {
    let h = home(Some("[sessions\narchive_days = SECRET\n"));
    let err = resolve(None, None, false, h.arg()).unwrap_err();
    assert!(
        matches!(
            &err,
            ConfigError::Key {
                key: "sessions.archive_days",
                ..
            }
        ),
        "{err:?}"
    );
    assert!(!err.to_string().contains("SECRET"), "{err}");
    // Restore passes days, so it reaches the DEST key and fails there.
    let err = resolve(Some(1), None, true, h.arg()).unwrap_err();
    assert!(
        matches!(
            &err,
            ConfigError::Key {
                key: "sessions.archive_dest",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn a_file_writable_by_others_is_an_error_for_the_keys_it_sets() {
    let h = home(Some("[sessions]\narchive_dest = \"/var/a\"\n"));
    fs::set_permissions(h.file(), fs::Permissions::from_mode(0o666)).unwrap();
    let err = resolve(None, None, false, h.arg()).unwrap_err();
    assert!(
        matches!(
            &err,
            ConfigError::Key {
                key: "sessions.archive_dest",
                ..
            }
        ),
        "{err:?}"
    );
    assert!(
        err.to_string().contains("writable by group or others"),
        "{err}"
    );
}

#[test]
fn flags_work_when_the_file_is_unusable() {
    let h = home(Some("[sessions]\narchive_days = 0\narchive_dest = 7\n"));
    let o = resolve(Some(5), Some(PathBuf::from("/var/c")), false, h.arg()).unwrap();
    assert_eq!((o.days, o.dest), (5, PathBuf::from("/var/c")));
}

#[test]
fn no_home_is_an_error() {
    for h in [None, Some(PathBuf::new())] {
        let err = resolve(None, None, false, h).unwrap_err();
        assert!(matches!(err, ConfigError::NoHome));
        assert_eq!(err.to_string(), "HOME is not set");
    }
}

#[test]
fn a_relative_dest_flag_is_made_absolute() {
    let h = home(None);
    let o = resolve(None, Some(PathBuf::from("rel/dir")), false, h.arg()).unwrap();
    assert!(o.dest.is_absolute(), "{}", o.dest.display());
    assert!(o.dest.ends_with("rel/dir"));
}

#[test]
fn the_test_guard_refuses_a_path_outside_the_temp_dir_and_names_it() {
    // Unit tests always run under the guard.
    assert_eq!(
        outside_test_temp("HOME", Path::new("/")).as_deref(),
        Some(
            "AMPLIHACK_TEST_HOME_GUARD is set and HOME (/) is outside the temp dir; nothing changed"
        )
    );
    // A path under the temp dir passes, whether or not it exists yet.
    let h = home(None);
    assert_eq!(outside_test_temp("HOME", &h.path), None);
    assert_eq!(
        outside_test_temp("DEST", &h.path.join("missing/dest")),
        None
    );
}
