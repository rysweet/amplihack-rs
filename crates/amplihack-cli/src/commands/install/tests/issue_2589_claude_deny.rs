//! Issue #2589 (TDD): the Claude Code half of the dangerous-command policy.
//!
//! The entries go into `permissions.deny` of `$HOME/.claude/settings.json`,
//! of every managed profile's `settings.json`, and of a profile seeded from a
//! home file written before this change (F6, F11, F12).
//!
//! Visibility this file needs: `commands::profile` re-exports `ProfileName`
//! as `pub(crate)` beside the existing `ProfileStore` re-export, so the
//! seeding test can create a profile.
//!
//! Every test here uses a temp HOME and a temp profile root. None of them
//! touches the real `~/.claude`, `~/.codex` or `~/.amplihack` (T1, #2586).

use super::super::settings::{
    PROFILE_REFRESH_TEST_OPT_IN, ensure_settings_json_for, refresh_profile_hooks,
};
use crate::commands::profile::ROOT_ENV;
use crate::policy;
use crate::test_support::{env_lock, tempdir_outside_framework_roots};
use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

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

fn hooks_file(path: &Path) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_path_buf()
}

/// `<root>/work/settings.json`, with `body` as its starting content.
fn profile_settings(root: &Path, body: &str) -> PathBuf {
    let profile = root.join("work");
    fs::create_dir_all(&profile).unwrap();
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    let settings = profile.join("settings.json");
    fs::write(&settings, body).unwrap();
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();
    settings
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

fn deny_entries(settings: &Value) -> Vec<String> {
    settings["permissions"]["deny"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// Writes `[policy] deny_commands = [...]` into `<home>/.amplihack/config.toml`.
fn instance_config(home: &Path, entries: &[&str]) {
    let dir = home.join(".amplihack");
    fs::create_dir_all(&dir).unwrap();
    let list = entries
        .iter()
        .map(|e| format!("\"{e}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let path = dir.join("config.toml");
    fs::write(&path, format!("[policy]\ndeny_commands = [{list}]\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

// --- the home settings.json (T2, T4, T5) ------------------------------------

/// Runs `ensure_settings_json_for` against a temp HOME and returns the path
/// of the settings file it wrote.
fn install_home_settings(home: &Path) -> PathBuf {
    let settings = home.join(".claude/settings.json");
    let staging = home.join("staging");
    fs::create_dir_all(&staging).unwrap();
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    ensure_settings_json_for(&settings, &staging, 0, &hooks, None, true)
        .expect("install writes the home settings file");
    settings
}

#[test]
fn install_writes_every_default_deny_entry_once_into_the_home_settings_file() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let _env = Env::set(&[("HOME", Some(&home))]);

    let settings = install_home_settings(&home);
    let deny = deny_entries(&read_json(&settings));
    for entry in policy::claude::deny_entries(&[]) {
        assert_eq!(
            deny.iter().filter(|e| **e == entry).count(),
            1,
            "{entry} must appear exactly once in {deny:?}"
        );
    }
}

#[test]
fn install_keeps_a_users_own_deny_entry_and_unrelated_settings_keys() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let _env = Env::set(&[("HOME", Some(&home))]);

    let claude = home.join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(
        claude.join("settings.json"),
        r#"{"language":"en","permissions":{"deny":["Bash(curl:*)"],"allow":["Bash"]}}"#,
    )
    .unwrap();

    let settings = install_home_settings(&home);
    let value = read_json(&settings);
    assert_eq!(value["language"], "en", "unrelated keys survive");
    assert_eq!(value["permissions"]["allow"], serde_json::json!(["Bash"]));
    let deny = deny_entries(&value);
    assert!(
        deny.contains(&"Bash(curl:*)".to_string()),
        "the user's own entry is kept: {deny:?}"
    );
    assert!(deny.contains(&"Bash(pkill:*)".to_string()), "{deny:?}");
}

#[test]
fn a_second_install_leaves_the_home_settings_file_byte_identical() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let _env = Env::set(&[("HOME", Some(&home))]);

    let settings = install_home_settings(&home);
    let before = fingerprint(&settings);
    let settings_again = install_home_settings(&home);
    assert_eq!(settings_again, settings);
    assert_eq!(
        fingerprint(&settings),
        before,
        "T5/D10: idempotence is proven by bytes, not by mtime alone"
    );
}

#[test]
fn an_instance_addition_appears_in_the_home_file_alongside_the_defaults() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    fs::create_dir_all(&home).unwrap();
    instance_config(&home, &["shutdown"]);
    let _env = Env::set(&[("HOME", Some(&home))]);

    let settings = install_home_settings(&home);
    let deny = deny_entries(&read_json(&settings));
    assert!(deny.contains(&"Bash(shutdown:*)".to_string()), "{deny:?}");
    assert!(
        deny.contains(&"Bash(pkill:*)".to_string()),
        "F5/R6: an addition never removes a default: {deny:?}"
    );
}

/// F3/A10: the deny list must not stop the ordinary push forms, including the
/// hooks-on push this very change is delivered by.
#[test]
fn no_written_entry_denies_an_ordinary_push() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let _env = Env::set(&[("HOME", Some(&home))]);

    let deny = deny_entries(&read_json(&install_home_settings(&home)));
    for entry in &deny {
        let prefix = entry
            .strip_prefix("Bash(")
            .and_then(|e| e.strip_suffix(":*)"))
            .unwrap_or_default();
        for ordinary in [
            "git push",
            "git push -u origin main",
            "git push origin HEAD:topic",
        ] {
            assert!(
                prefix.is_empty()
                    || !(ordinary == prefix || ordinary.starts_with(&format!("{prefix} "))),
                "{entry} would deny {ordinary}"
            );
        }
    }
}

// --- managed profiles (T3, T4, T5) ------------------------------------------

#[test]
fn profile_refresh_writes_the_policy_into_each_profile_settings_json() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let root = home.join("profiles");
    let settings = profile_settings(&root, r#"{"language":"en","hooks":{}}"#);
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);

    refresh_profile_hooks(&hooks);
    let value = read_json(&settings);
    assert_eq!(value["language"], "en", "other settings are kept");
    let deny = deny_entries(&value);
    for entry in policy::claude::deny_entries(&[]) {
        assert_eq!(
            deny.iter().filter(|e| **e == entry).count(),
            1,
            "{entry} once in {deny:?}"
        );
    }
}

#[test]
fn a_profiles_own_deny_entries_survive_the_refresh() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let root = home.join("profiles");
    let settings = profile_settings(
        &root,
        r#"{"permissions":{"deny":["Bash(curl:*)"],"defaultMode":"acceptEdits"},"hooks":{}}"#,
    );
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);

    refresh_profile_hooks(&hooks);
    let value = read_json(&settings);
    assert_eq!(
        value["permissions"]["defaultMode"], "acceptEdits",
        "the profile's own mode is not changed by the policy writer"
    );
    let deny = deny_entries(&value);
    assert!(deny.contains(&"Bash(curl:*)".to_string()), "{deny:?}");
    assert!(deny.contains(&"Bash(killall:*)".to_string()), "{deny:?}");
}

#[test]
fn a_second_profile_refresh_leaves_the_file_byte_identical() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let root = home.join("profiles");
    let settings = profile_settings(&root, r#"{"language":"en","hooks":{}}"#);
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);

    refresh_profile_hooks(&hooks);
    let before = fingerprint(&settings);
    refresh_profile_hooks(&hooks);
    assert_eq!(fingerprint(&settings), before);
}

/// SEC-C2/D9: a profile whose `permissions` is not an object is reported and
/// left exactly as found, not replaced. Overwriting it would be a new way to
/// discard a profile's own permissions.
#[test]
fn a_profile_with_a_malformed_permissions_value_is_left_as_found() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let root = home.join("profiles");
    let body = r#"{"permissions":"bypassPermissions","hooks":{}}"#;
    let settings = profile_settings(&root, body);
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);

    refresh_profile_hooks(&hooks);
    let value = read_json(&settings);
    assert_eq!(
        value["permissions"], "bypassPermissions",
        "the malformed value is kept, not replaced"
    );
}

/// D9: a profile directory with no `settings.json` does not get one.
#[test]
fn a_profile_without_a_settings_file_does_not_gain_one() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let root = home.join("profiles");
    fs::create_dir_all(root.join("bare")).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);

    refresh_profile_hooks(&hooks);
    assert!(!root.join("bare/settings.json").exists());
}

// --- seeding a new profile (F6, D8) -----------------------------------------

/// A profile seeded from a home file written before this change copies no
/// deny entries, so seeding must write the policy itself rather than rely on
/// `SHARED_SETTINGS_KEYS` copying `permissions`.
#[test]
fn seeding_a_new_profile_writes_the_policy_even_from_a_pre_change_home_file() {
    use crate::commands::profile::{ProfileName, ProfileStore};

    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let source = home.join(".claude");
    fs::create_dir_all(&source).unwrap();
    // The home file as it looked before this change: no deny entries at all.
    fs::write(
        source.join("settings.json"),
        r#"{"language":"en","permissions":{"allow":["Bash"]}}"#,
    )
    .unwrap();
    let root = home.join("profiles");
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let _env = Env::set(&[("HOME", Some(&home))]);

    let store = ProfileStore::new(root.clone()).with_shared_assets(source);
    let name = ProfileName::parse("fresh").unwrap();
    let dir = store.add(&name).expect("the profile is created");

    let value = read_json(&dir.join("settings.json"));
    assert_eq!(value["language"], "en");
    let deny = deny_entries(&value);
    assert!(
        deny.contains(&"Bash(pkill:*)".to_string()),
        "a newly seeded profile carries the policy: {deny:?}"
    );
    assert!(
        deny.contains(&"Bash(tmux kill-server:*)".to_string()),
        "{deny:?}"
    );
}
