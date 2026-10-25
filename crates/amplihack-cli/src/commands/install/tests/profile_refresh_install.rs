//! Issue #1661: `amplihack install` gives each profile's settings.json the
//! hook registrations it writes to ~/.claude/settings.json. Uses a temp
//! profile root only.

use super::super::settings::{PROFILE_REFRESH_TEST_OPT_IN, refresh_profile_hooks};
use crate::commands::profile::ROOT_ENV;
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
    let profile = root.join("work");
    fs::create_dir_all(&profile).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let settings = profile.join("settings.json");
    fs::write(&settings, r#"{"language":"en","hooks":{}}"#).unwrap();
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();
    (d, settings)
}

#[test]
fn install_registers_this_builds_hooks_in_each_profile() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let (d, settings) = root_with_profile();
    let home = d.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let root = d.path().join("profiles");
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);
    refresh_profile_hooks(Path::new("/opt/amplihack/amplihack-hooks"));
    let s: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(s["language"], "en", "other settings are kept");
    let stop_failure = s["hooks"]["StopFailure"].to_string();
    assert!(stop_failure.contains("limit-resume"), "{stop_failure}");
    assert!(s["hooks"]["UserPromptSubmit"].is_array());
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
