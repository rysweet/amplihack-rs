//! Issue #1661: `amplihack install` gives each profile's settings.json the
//! hook registrations it writes to ~/.claude/settings.json. Uses a temp
//! profile root only.

use super::super::settings::{PROFILE_REFRESH_TEST_OPT_IN, refresh_profile_hooks};
use super::helpers::create_exe_stub;
use crate::commands::profile::ROOT_ENV;
use crate::test_support::{EnvGuard, env_lock};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Sets the root, HOME and the install opt-in (removed when `opt_in` is
/// false). The guard restores all three on drop, also on panic.
fn env_for(root: &Path, home: &Path, opt_in: bool) -> EnvGuard {
    let guard = EnvGuard::capture(&[ROOT_ENV, "HOME", PROFILE_REFRESH_TEST_OPT_IN]);
    // SAFETY: edition 2024 requires unsafe; callers hold env_lock().
    unsafe {
        std::env::set_var(ROOT_ENV, root);
        std::env::set_var("HOME", home);
        if opt_in {
            std::env::set_var(PROFILE_REFRESH_TEST_OPT_IN, "1");
        } else {
            std::env::remove_var(PROFILE_REFRESH_TEST_OPT_IN);
        }
    }
    guard
}

/// A temp HOME holding an existing hook stub and a profile root, so the
/// #1699 check (existing binary; root inside HOME) lets the refresh run.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let f = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        fs::create_dir_all(f.home()).unwrap();
        create_exe_stub(&f.home().join(".local/bin"), "amplihack-hooks");
        f.add_profile(&f.root(), "work", r#"{"language":"en","hooks":{}}"#);
        f
    }
    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }
    fn root(&self) -> PathBuf {
        self.home().join(".claude-profiles")
    }
    fn bin(&self) -> PathBuf {
        self.home().join(".local/bin/amplihack-hooks")
    }
    fn settings(&self) -> PathBuf {
        self.root().join("work/settings.json")
    }
    fn add_profile(&self, root: &Path, name: &str, text: &str) -> PathBuf {
        let profile = root.join(name);
        fs::create_dir_all(&profile).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
        let settings = profile.join("settings.json");
        fs::write(&settings, text).unwrap();
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();
        settings
    }
}

#[test]
fn install_registers_this_builds_hooks_in_each_profile() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = Fixture::new();
    assert!(f.root().starts_with(f.dir.path()));
    let _env = env_for(&f.root(), &f.home(), true);
    refresh_profile_hooks(&f.bin());
    let s: Value = serde_json::from_str(&fs::read_to_string(f.settings()).unwrap()).unwrap();
    assert_eq!(s["language"], "en", "other settings are kept");
    let stop_failure = s["hooks"]["StopFailure"].to_string();
    assert!(stop_failure.contains("limit-resume"), "{stop_failure}");
    assert!(s["hooks"]["UserPromptSubmit"].is_array());
    let hooks = s["hooks"].to_string();
    assert!(hooks.contains(&f.bin().display().to_string()), "{hooks}");
}

#[test]
fn unit_tests_never_touch_profiles_without_opting_in() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = Fixture::new();
    let _env = env_for(&f.root(), &f.home(), false);
    let before = fs::read_to_string(f.settings()).unwrap();
    refresh_profile_hooks(&f.bin());
    assert_eq!(fs::read_to_string(f.settings()).unwrap(), before);
}

/// Issue #1699: install run with a temp HOME and an inherited root outside
/// it must leave that root's profiles byte-for-byte alone.
#[test]
fn install_with_a_temp_home_leaves_a_root_outside_home_untouched() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = Fixture::new();
    let real = f.dir.path().join("real/profiles");
    let text = r#"{"language":"en","hooks":{"Stop":[{"hooks":[{"type":"command","command":"/opt/old/amplihack-hooks stop"}]}]}}"#;
    let settings = f.add_profile(&real, "work", text);
    assert!(real.starts_with(f.dir.path()) && !real.starts_with(f.home()));
    let mtime = fs::metadata(&settings).unwrap().modified().unwrap();
    let _env = env_for(&real, &f.home(), true);
    refresh_profile_hooks(&f.bin());
    assert_eq!(fs::read_to_string(&settings).unwrap(), text);
    assert_eq!(fs::metadata(&settings).unwrap().modified().unwrap(), mtime);
    let entries: Vec<_> = fs::read_dir(real.join("work"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["settings.json"], "no temp, lock or backup file");
}

/// Issue #1699 (AC4): the refresh moves hooks to the new binary and leaves
/// the profile's `statusLine` exactly as it was.
#[test]
fn install_refresh_keeps_a_profile_status_line() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let f = Fixture::new();
    let status = serde_json::json!({
        "type": "command",
        "command": "/usr/local/bin/amplihack profile statusline --profile work",
    });
    let text = serde_json::json!({
        "statusLine": status,
        "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/opt/old/amplihack-hooks stop"}]}]},
    })
    .to_string();
    let settings = f.add_profile(&f.root(), "status", &text);
    let _env = env_for(&f.root(), &f.home(), true);
    refresh_profile_hooks(&f.bin());
    let s: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(s["statusLine"], status);
    let stop = s["hooks"]["Stop"].to_string();
    assert!(stop.contains(&f.bin().display().to_string()), "{stop}");
    assert!(!stop.contains("/opt/old/amplihack-hooks"), "{stop}");
}
