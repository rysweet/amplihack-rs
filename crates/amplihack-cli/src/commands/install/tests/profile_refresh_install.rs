//! Issue #1661: `amplihack install` gives each profile's settings.json the
//! hook registrations it writes to ~/.claude/settings.json. Uses a temp
//! profile root only.
//!
//! Issue #1699: a hooks path under a temporary directory is never written
//! into a profile root outside HOME, and a hooks path that does not exist is
//! never written at all.

use super::super::settings::{PROFILE_REFRESH_TEST_OPT_IN, refresh_profile_hooks};
use crate::commands::profile::{ROOT_ENV, Refresh, refresh_profile_settings};
use crate::test_support::env_lock;
use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Sets environment variables and restores them on drop.
struct Env(Vec<(&'static str, Option<OsString>)>);

impl Env {
    fn set(vars: &[(&'static str, Option<&Path>)]) -> Self {
        let mut previous = Vec::new();
        for (key, value) in vars {
            previous.push((*key, std::env::var_os(key)));
            // SAFETY: edition 2024 requires unsafe; callers hold env_lock().
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self(previous)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            // SAFETY: as in `set`.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn root_with_profile() -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("profiles");
    let settings = profile_settings(&root);
    (d, settings)
}

/// Creates `<root>/work/settings.json` and returns its path.
fn profile_settings(root: &Path) -> std::path::PathBuf {
    let profile = root.join("work");
    fs::create_dir_all(&profile).unwrap();
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    let settings = profile.join("settings.json");
    fs::write(&settings, r#"{"language":"en","hooks":{}}"#).unwrap();
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();
    settings
}

/// Writes an executable stand-in for `amplihack-hooks` at `path`.
fn hooks_file(path: &Path) -> std::path::PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_path_buf()
}

/// Bytes, mtime and inode: a rewrite, even to the same bytes, changes one.
fn fingerprint(path: &Path) -> (Vec<u8>, std::time::SystemTime, u64) {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path).unwrap();
    (
        fs::read(path).unwrap(),
        meta.modified().unwrap(),
        meta.ino(),
    )
}

#[test]
fn install_registers_this_builds_hooks_in_each_profile() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    // #1699 (D4): the root lies inside the temp HOME, and the hooks binary is
    // a real file there, so the fixture is self-contained and the refresh is
    // allowed.
    let root = home.join("profiles");
    let settings = profile_settings(&root);
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);
    refresh_profile_hooks(&hooks);
    let s: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(s["language"], "en", "other settings are kept");
    let stop_failure = s["hooks"]["StopFailure"].to_string();
    assert!(stop_failure.contains("limit-resume"), "{stop_failure}");
    assert!(s["hooks"]["UserPromptSubmit"].is_array());
    assert!(
        s["hooks"]
            .to_string()
            .contains(&hooks.display().to_string()),
        "the registered command is this hooks binary"
    );
}

/// T1, the incident: HOME is temp dir A with the hooks binary inside it, and
/// the profile root is a separate directory B. Nothing under B is written.
#[test]
fn refresh_under_temp_home_never_writes_root_outside_it() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let a = tempfile::tempdir().unwrap();
    let home = a.path().join("home");
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let b = tempfile::tempdir().unwrap();
    let root = b.path().join("claude-profiles");
    let settings = profile_settings(&root);
    fs::create_dir_all(root.join("bare")).unwrap();
    let before = fingerprint(&settings);
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);

    refresh_profile_hooks(&hooks);
    assert_eq!(fingerprint(&settings), before, "settings.json untouched");
    assert!(
        !root.join("bare/settings.json").exists(),
        "no settings.json is created"
    );

    let results = refresh_profile_settings(&hooks, &|s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([]);
    });
    assert_eq!(fingerprint(&settings), before, "settings.json untouched");
    let work = &results.iter().find(|(n, _)| n == "work").unwrap().1;
    match work {
        // Either check refuses the incident: the throwaway-HOME check runs
        // first when the passwd home is known (it differs from this HOME),
        // the temp-directory check otherwise.
        Refresh::Skipped(reason) => {
            assert!(
                reason.contains("is not the account's home")
                    || reason.contains("is under temporary directory"),
                "{reason}"
            );
            assert!(reason.contains("is outside HOME"), "{reason}");
        }
        other => panic!("expected Skipped, got {other:?}"),
    }
    let bare = &results.iter().find(|(n, _)| n == "bare").unwrap().1;
    assert_eq!(bare, &Refresh::Missing, "a profile without settings.json");
}

/// T1 with HOME unset: a temp hooks path is still refused.
#[test]
fn refresh_with_home_unset_and_temp_hooks_is_refused() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let a = tempfile::tempdir().unwrap();
    let hooks = hooks_file(&a.path().join("bin/amplihack-hooks"));
    let root = a.path().join("profiles");
    let settings = profile_settings(&root);
    let before = fingerprint(&settings);
    let _env = Env::set(&[(ROOT_ENV, Some(&root)), ("HOME", None)]);
    let results = refresh_profile_settings(&hooks, &|s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([]);
    });
    assert_eq!(fingerprint(&settings), before);
    assert!(
        matches!(&results[..], [(_, Refresh::Skipped(r))] if r.contains("HOME is unset")),
        "{results:?}"
    );
}

/// T2 through the production entry point: a hooks path that does not exist
/// is never registered, even in a self-contained fixture.
#[test]
fn a_missing_hooks_binary_never_reaches_profiles() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    let root = home.join("profiles");
    let settings = profile_settings(&root);
    let before = fingerprint(&settings);
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);
    let missing = Path::new("/nonexistent/amplihack-hooks");
    refresh_profile_hooks(missing);
    assert_eq!(fingerprint(&settings), before);
    let results = refresh_profile_settings(missing, &|s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([]);
    });
    assert!(
        matches!(&results[..], [(_, Refresh::Skipped(r))] if r.contains("does not exist")),
        "{results:?}"
    );
}

#[test]
fn unit_tests_never_touch_profiles_without_opting_in() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let (d, settings) = root_with_profile();
    let root = d.path().join("profiles");
    let _env = Env::set(&[(ROOT_ENV, Some(&root)), (PROFILE_REFRESH_TEST_OPT_IN, None)]);
    let before = fs::read_to_string(&settings).unwrap();
    refresh_profile_hooks(Path::new("/opt/amplihack/amplihack-hooks"));
    assert_eq!(fs::read_to_string(&settings).unwrap(), before);
}
