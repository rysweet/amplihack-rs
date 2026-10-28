//! Issue #1661: `amplihack install` gives each profile's settings.json the
//! hook registrations it writes to ~/.claude/settings.json. Uses a temp
//! profile root only.
//!
//! Issue #1699: an install under a temp HOME must not write its temp hook
//! path into a profile root outside the temp dir, whichever way that root was
//! found. `TMPDIR` is pointed at `test_support::fake_system_tmp_base()/tmp`,
//! so a per-test dir elsewhere under that base is a "real" root outside the
//! temp dir. Nothing here touches the real HOME.

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

/// A 0755 `amplihack-hooks` stub at `<home>/.local/bin`.
fn hooks_stub(home: &Path) -> std::path::PathBuf {
    let bin = home.join(".local/bin/amplihack-hooks");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    bin
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
    let bin = hooks_stub(&home);
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);
    refresh_profile_hooks(&bin);
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
    let bin = hooks_stub(&d.path().join("home"));
    let before = fs::read_to_string(&settings).unwrap();
    refresh_profile_hooks(&bin);
    assert_eq!(fs::read_to_string(&settings).unwrap(), before);
}

// ---- Issue #1699 -----------------------------------------------------------

use crate::commands::profile::{Refresh, refresh_profile_settings_guarded};
use std::collections::BTreeMap;
use std::path::PathBuf;

const SETTINGS_WITH_HOOKS: &str = r#"{
  "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "\"/home/dev/.local/bin/amplihack-hooks\" stop"}]}]},
  "statusLine": {"type": "command", "command": "amplihack profile statusline -- ccstatusline"}
}"#;

/// `tmp` is the temp dir, `<tmp>/<case>/home` the temp HOME with a hooks
/// stub, and `<base>/<case>/real-root` a profile root outside the temp dir.
struct Outside {
    _temp_case: tempfile::TempDir,
    _outside_case: tempfile::TempDir,
    tmp: PathBuf,
    home: PathBuf,
    bin: PathBuf,
    root: PathBuf,
}

fn outside_layout() -> Outside {
    let base = crate::test_support::fake_system_tmp_base();
    let tmp = base.join("tmp");
    let temp_case = tempfile::tempdir_in(&tmp).unwrap();
    let outside_case = tempfile::tempdir_in(base).unwrap();
    let home = temp_case.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let bin = hooks_stub(&home);
    let root = outside_case.path().join("real-root");
    for name in ["work", "personal"] {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        fs::write(&file, SETTINGS_WITH_HOOKS).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    Outside {
        _temp_case: temp_case,
        _outside_case: outside_case,
        tmp,
        home,
        bin,
        root,
    }
}

/// Every file under `dir`, with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    out
}

fn assert_all_skipped_naming(results: &[(String, Refresh)], bin: &Path) {
    assert_eq!(results.len(), 2, "{results:?}");
    for (name, result) in results {
        match result {
            Refresh::Skipped(reason) => {
                assert!(reason.contains(&format!("{bin:?}")), "{name}: {reason}");
                assert!(
                    reason.contains("is under temporary dir"),
                    "{name}: {reason}"
                );
            }
            other => panic!("{name}: expected Skipped, got {other:?}"),
        }
    }
}

/// T1a: the root is reached through an inherited AMPLIHACK_CLAUDE_PROFILE_ROOT.
#[test]
fn temp_home_install_never_writes_an_outside_root_from_the_env_var() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let o = outside_layout();
    let _env = Env::set(&[
        ("TMPDIR", Some(&o.tmp)),
        ("HOME", Some(&o.home)),
        (ROOT_ENV, Some(&o.root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
    ]);
    let before = snapshot(&o.root);
    refresh_profile_hooks(&o.bin);
    assert_eq!(snapshot(&o.root), before, "refresh rewrote a real profile");
    let results = refresh_profile_settings_guarded(&o.bin, &|s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([{"matcher": "rate_limit"}]);
    });
    assert_all_skipped_naming(&results, &o.bin);
    assert_eq!(snapshot(&o.root), before);
}

/// T1b: the root is reached through `<temp HOME>/.amplihack/profile-root`.
#[test]
fn temp_home_install_never_writes_an_outside_root_from_the_saved_file() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let o = outside_layout();
    let config = o.home.join(".amplihack");
    fs::create_dir_all(&config).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o700)).unwrap();
    let saved = config.join("profile-root");
    fs::write(&saved, format!("{}\n", o.root.display())).unwrap();
    fs::set_permissions(&saved, fs::Permissions::from_mode(0o600)).unwrap();
    let _env = Env::set(&[
        ("TMPDIR", Some(&o.tmp)),
        ("HOME", Some(&o.home)),
        (ROOT_ENV, None),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
    ]);
    let before = snapshot(&o.root);
    refresh_profile_hooks(&o.bin);
    assert_eq!(snapshot(&o.root), before, "refresh rewrote a real profile");
    let results = refresh_profile_settings_guarded(&o.bin, &|s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([{"matcher": "rate_limit"}]);
    });
    assert_all_skipped_naming(&results, &o.bin);
    assert_eq!(snapshot(&o.root), before);
}

/// T2: a hooks binary that does not exist is never written, even into a
/// profile under the temp dir.
#[test]
fn a_missing_hooks_binary_is_never_written_into_a_profile() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let (d, settings) = root_with_profile();
    let root = d.path().join("profiles");
    let home = d.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let missing = home.join(".local/bin/amplihack-hooks");
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);
    let before = fs::read(&settings).unwrap();
    refresh_profile_hooks(&missing);
    assert_eq!(fs::read(&settings).unwrap(), before);
    let results = refresh_profile_settings_guarded(&missing, &|s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([]);
    });
    assert_eq!(
        results,
        vec![(
            "work".to_string(),
            Refresh::Skipped(format!("hooks binary {missing:?} does not exist"))
        )]
    );
    assert_eq!(fs::read(&settings).unwrap(), before);
}

/// T3: a refresh that is allowed keeps the auto-switch `statusLine` exactly.
#[test]
fn an_allowed_refresh_keeps_the_status_line() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempfile::tempdir().unwrap();
    let base = d.path().canonicalize().unwrap();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    let bin = hooks_stub(&home);
    let root = base.join("profiles");
    let dir = root.join("work");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let file = dir.join("settings.json");
    fs::write(&file, SETTINGS_WITH_HOOKS).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let status_before: Value =
        serde_json::from_str::<Value>(SETTINGS_WITH_HOOKS).unwrap()["statusLine"].clone();
    // Profile and binary are both under the real temp dir: allowed.
    let _env = Env::set(&[
        ("TMPDIR", None),
        ("HOME", Some(&home)),
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
    ]);
    let results = refresh_profile_settings_guarded(&bin, &|s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([{"matcher": "rate_limit"}]);
    });
    assert_eq!(results, vec![("work".to_string(), Refresh::Updated)]);
    let after: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(after["statusLine"], status_before);
}

/// AC4 normal case: an install whose binary and profiles are both outside
/// the temp dir still refreshes the profiles.
#[test]
fn a_real_install_still_refreshes_profiles_outside_temp() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fake = crate::test_support::fake_system_tmp_base();
    // `<fake>/tmp` is the temp dir; HOME, binary and root are all outside it.
    let tmp = fake.join("tmp");
    let d = tempfile::tempdir_in(fake).unwrap();
    let base = d.path();
    let home = base.join("home");
    let bin = hooks_stub(&home);
    let root = base.join("profiles");
    let dir = root.join("work");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let file = dir.join("settings.json");
    fs::write(&file, r#"{"language":"en","hooks":{}}"#).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let _env = Env::set(&[
        ("TMPDIR", Some(&tmp)),
        ("HOME", Some(&home)),
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
    ]);
    refresh_profile_hooks(&bin);
    let s: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert!(s["hooks"]["UserPromptSubmit"].is_array(), "{s}");
    assert!(
        s["hooks"].to_string().contains(&bin.display().to_string()),
        "{s}"
    );
}
