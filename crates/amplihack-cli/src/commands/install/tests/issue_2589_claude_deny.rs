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
//! Every test here uses a temp HOME and a temp profile root, and unsets
//! `CODEX_HOME`, which `codex_home()` prefers over `$HOME/.codex`: without that
//! a machine where it is set would have these tests write the rules file into
//! the developer's real Codex home. None of them touches the real
//! `~/.claude`, `~/.codex` or `~/.amplihack` (T1, #2586).

use super::super::settings::{
    PROFILE_REFRESH_TEST_OPT_IN, ensure_settings_json_for, refresh_profile_hooks,
};
use crate::commands::profile::ROOT_ENV;
use crate::policy;
use crate::test_support::{env_lock, env_paths, tempdir_outside_framework_roots};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

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
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

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
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

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
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

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
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

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
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

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
    let _env = env_paths(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
        ("CODEX_HOME", None),
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
    let _env = env_paths(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
        ("CODEX_HOME", None),
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
    let _env = env_paths(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
        ("CODEX_HOME", None),
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
    let _env = env_paths(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
        ("CODEX_HOME", None),
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
    let _env = env_paths(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
        ("CODEX_HOME", None),
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
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

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

// --- a settings file that cannot take the entries (SEC-C3) ------------------
//
// The defect these tests exist for: a settings file holding a `permissions`
// value amplihack cannot merge gets no deny entries at all, on every install
// and every update, while the install output said the rules had been
// written. The reason reached `tracing::warn!` only, which an ordinary
// install does not show. Silent non-installation of a safety control is
// worse than visible failure, so each writer records what it met and the
// install's policy section carries the warning on its own channel.
//
// Expected seams:
//
// ```ignore
// // install/settings.rs: the reason reaches the caller as well as the record
// pub(crate) fn apply_deny_policy(settings: &mut Value, label: &str) -> Option<String>;
//
// // profile/settings_refresh.rs: the refresh closure is given the profile
// // name, because a warning that does not say which profile cannot be acted
// // on
// pub(crate) fn refresh_profile_settings(
//     hooks_bin: &Path,
//     update: &dyn Fn(&str, &mut Value),
// ) -> Vec<(String, Refresh)>;
// ```
//
// The labels are the ones `docs/reference/dangerous-command-policy.md`
// prints: `home settings`, `profile <name>`, `new profile <name>`.

use super::super::apply_dangerous_command_policy;
use crate::policy::{HarnessWrite, ReportLine};

/// Empties the Claude policy record left by any earlier call on this thread.
fn clear_policy_record() {
    let _ = policy::claude::take_record();
}

/// Whether the report claims `harness` had its rules written to a file.
fn claims_written(lines: &[ReportLine], harness: &str) -> bool {
    lines
        .iter()
        .any(|line| line.text.starts_with(harness) && line.text.contains("deny rules written to"))
}

/// A home settings file whose `permissions.deny` is not an array: the one
/// shape `ensure_permissions` leaves in place today, and the one a hand edit
/// most often produces.
#[test]
fn a_home_file_whose_deny_is_not_an_array_is_left_as_found_and_warns() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let claude = home.join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(
        claude.join("settings.json"),
        r#"{"language":"en","permissions":{"deny":"Bash(curl:*)"}}"#,
    )
    .unwrap();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

    let settings = install_home_settings(&home);
    let value = read_json(&settings);
    assert_eq!(
        value["permissions"]["deny"], "Bash(curl:*)",
        "a value amplihack cannot merge is reported, never replaced"
    );

    let report = apply_dangerous_command_policy();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("Claude Code") && w.contains("home settings")),
        "the install's own channel names the file it could not write: {:?}",
        report.warnings
    );
    assert!(
        !claims_written(&report.lines, "Claude Code"),
        "and the same run does not claim Claude coverage: {:?}",
        report.lines
    );
}

/// The `permissions` key itself holding a scalar. The reference page says
/// this file is left byte-identical and gets no entries; today the settings
/// writer replaces it before the policy writer sees it, which both discards
/// the user's value and hides the failure.
#[test]
fn a_home_file_whose_permissions_is_not_an_object_is_left_as_found_and_warns() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let claude = home.join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(
        claude.join("settings.json"),
        r#"{"language":"en","permissions":"bypassPermissions"}"#,
    )
    .unwrap();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

    let settings = install_home_settings(&home);
    let value = read_json(&settings);
    assert_eq!(
        value["permissions"], "bypassPermissions",
        "the user's own value is kept, not replaced with amplihack's defaults"
    );

    let report = apply_dangerous_command_policy();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("Claude Code") && w.contains("home settings")),
        "{:?}",
        report.warnings
    );
    assert!(
        !claims_written(&report.lines, "Claude Code"),
        "{:?}",
        report.lines
    );
}

/// The warning names a JSON key or an I/O error kind. It never quotes a value
/// read out of the file, which is content amplihack did not write (SEC-V4).
#[test]
fn a_claude_warning_never_quotes_a_value_read_out_of_the_file() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let claude = home.join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(
        claude.join("settings.json"),
        r#"{"permissions":{"deny":"SECRET-VALUE-9f2a"}}"#,
    )
    .unwrap();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

    install_home_settings(&home);
    let report = apply_dangerous_command_policy();
    for warning in &report.warnings {
        assert!(
            !warning.contains("SECRET-VALUE-9f2a"),
            "a warning must not echo file content: {warning}"
        );
        assert!(
            !warning.chars().any(char::is_control),
            "and must stay on one line: {warning}"
        );
    }
}

/// A run where every file took the entries says so, and says nothing else.
#[test]
fn a_run_that_wrote_the_home_file_reports_claude_as_written_without_a_warning() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

    install_home_settings(&home);
    let report = apply_dangerous_command_policy();
    assert!(
        report.warnings.is_empty(),
        "nothing was refused: {:?}",
        report.warnings
    );
    assert!(
        claims_written(&report.lines, "Claude Code"),
        "{:?}",
        report.lines
    );
}

/// A refused profile is named, because a warning that does not say which
/// profile cannot be acted on.
#[test]
fn a_refused_profile_is_named_in_the_install_warning() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let root = home.join("profiles");
    profile_settings(&root, r#"{"permissions":"bypassPermissions","hooks":{}}"#);
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let _env = env_paths(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
        ("CODEX_HOME", None),
    ]);

    refresh_profile_hooks(&hooks);
    let report = apply_dangerous_command_policy();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("Claude Code") && w.contains("profile work")),
        "the warning names the profile it met: {:?}",
        report.warnings
    );
}

/// One refused profile beside a written home file is a partial write, not a
/// complete one and not a total failure.
#[test]
fn one_refused_profile_beside_a_written_home_file_reports_a_partial_write() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let root = home.join("profiles");
    profile_settings(&root, r#"{"permissions":"bypassPermissions","hooks":{}}"#);
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let _env = env_paths(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
        ("CODEX_HOME", None),
    ]);

    install_home_settings(&home);
    refresh_profile_hooks(&hooks);
    let report = apply_dangerous_command_policy();
    let claude = report
        .lines
        .iter()
        .find(|line| line.text.starts_with("Claude Code:"))
        .expect("a partial write is still reported");
    assert!(
        !claude.text.contains("deny rules written to"),
        "a partial write must not read as a complete one: {:?}",
        claude.text
    );
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
}

/// Seeding runs off the install path, on `amplihack profile add`. A file it
/// cannot write is recorded the same way, so the failure is visible there too
/// rather than only in a log the user never sees.
#[test]
fn seeding_a_profile_it_cannot_write_records_the_refusal() {
    use crate::commands::profile::{ProfileName, ProfileStore};

    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let source = home.join(".claude");
    fs::create_dir_all(&source).unwrap();
    // A home file whose `permissions` is copied into the new profile and
    // cannot then take the entries.
    fs::write(
        source.join("settings.json"),
        r#"{"language":"en","permissions":"bypassPermissions"}"#,
    )
    .unwrap();
    let root = home.join("profiles");
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

    let store = ProfileStore::new(root.clone()).with_shared_assets(source);
    let name = ProfileName::parse("fresh").unwrap();
    let dir = store
        .add(&name)
        .expect("a settings file that cannot take the entries does not fail the add");

    let value = read_json(&dir.join("settings.json"));
    assert_eq!(
        value["permissions"], "bypassPermissions",
        "the seeded value is kept, not replaced"
    );

    let (outcome, warnings) = policy::claude::take_record();
    assert_eq!(outcome, HarnessWrite::AllRefused, "{warnings:?}");
    assert!(
        warnings.iter().any(|w| w.contains("new profile fresh")),
        "the seeding path names the profile it seeded: {warnings:?}"
    );
}

/// Structural: seeding writes its warning where a person running `amplihack
/// profile add` will see it. Without this the record is set and nothing ever
/// prints it on that path.
#[test]
fn the_seeding_path_writes_its_warning_to_stderr() {
    const SOURCE: &str = include_str!("../../profile/store.rs");
    let seeding = SOURCE
        .split("fn seed_policy_into")
        .nth(1)
        .expect("seed_policy_into is defined in profile/store.rs");
    // To the end of this function, not a fixed number of bytes: the check is
    // "the seeding path reaches stderr", which a comment reworded above it
    // must not decide. `    }` at the method's indent closes it.
    let body = seeding
        .split_once("\n    }\n")
        .expect("seed_policy_into is a closed method")
        .0;
    assert!(
        body.contains("eprintln!"),
        "a refusal met while seeding must reach stderr, not only tracing:\n{body}"
    );
}

// --- a file the merge refused before the policy writer ran (SEC-C3) ---------
//
// `apply_deny_policy` runs inside the closure `hook_block::update_file` is
// given, and the merge decides whether to run that closure. Every path that
// refuses a file before the closure runs leaves the file absent from the
// record, and an absent file reads as "this harness has no file" -- a
// positive, and wrong, claim. The two tests below are the reachable shapes:
// a hand-edited profile with a trailing comma, and a dotfile-managed symlink.

/// A profile whose `settings.json` is not JSON is refused by the merge
/// before the policy writer runs. Beside a home file that did take the
/// entries, that is a partial write with the profile named, not a silent
/// complete one.
#[test]
fn a_profile_the_merge_refused_before_the_writer_ran_is_still_reported() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let root = home.join("profiles");
    // Not JSON: `update_file` returns `Unparseable` and never runs the
    // closure that would have applied the policy.
    profile_settings(&root, "{ not json");
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let _env = env_paths(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
        ("CODEX_HOME", None),
    ]);

    install_home_settings(&home);
    refresh_profile_hooks(&hooks);
    let report = apply_dangerous_command_policy();

    assert_eq!(
        fs::read_to_string(root.join("work/settings.json")).unwrap(),
        "{ not json",
        "the file was refused, so it is unchanged"
    );
    assert!(
        !claims_written(&report.lines, "Claude Code"),
        "one profile took no entries, so this is not a complete write: {:?}",
        report.lines
    );
    assert_eq!(
        report.warnings.len(),
        1,
        "the refused profile is warned about once: {:?}",
        report.warnings
    );
    assert!(
        report.warnings[0].contains("profile work"),
        "the warning names the profile it met: {:?}",
        report.warnings
    );
}

/// A symlinked home `settings.json` is refused by the merge before the
/// policy writer runs. The record must still hold it, or the report claims
/// there is no Claude settings file on this machine while one exists and
/// took no entries.
#[test]
fn a_symlinked_home_settings_file_is_reported_as_refused_not_as_absent() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let claude = home.join(".claude");
    fs::create_dir_all(&claude).unwrap();
    let elsewhere = d.path().join("elsewhere.json");
    fs::write(&elsewhere, "{}\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, claude.join("settings.json")).unwrap();
    let _env = env_paths(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

    install_home_settings(&home);
    let report = apply_dangerous_command_policy();

    assert_eq!(
        fs::read_to_string(&elsewhere).unwrap(),
        "{}\n",
        "a symlinked settings file is left as it was found"
    );
    let claude_line = report
        .lines
        .iter()
        .find(|line| line.text.starts_with("Claude Code:"));
    assert!(
        claude_line.is_none(),
        "every Claude file was refused, so nothing is claimed: {claude_line:?}"
    );
    assert_eq!(
        report.warnings.len(),
        1,
        "the refused home file is warned about once: {:?}",
        report.warnings
    );
    assert!(
        report.warnings[0].contains("home settings"),
        "the warning names the file it met: {:?}",
        report.warnings
    );
}

/// Structural: every arm in `install/settings.rs` that turns a merge result
/// into a refusal records it. `record` is called from inside the merge
/// closure, so an arm that returns before the closure runs and does not
/// record leaves a refused file looking like a file that does not exist.
/// Without this check that contract is a comment, and it was already broken
/// once at two of the four sites.
#[test]
fn every_refusal_arm_in_the_install_writer_records_what_it_refused() {
    const SOURCE: &str = include_str!("../settings.rs");
    let arms = [
        "FileMerge::Refused(reason) => {",
        "Refresh::Skipped(reason) = result {",
    ];
    for arm in arms {
        let after = SOURCE
            .split(arm)
            .nth(1)
            .unwrap_or_else(|| panic!("{arm} is still a match arm in install/settings.rs"));
        // To the end of the block, so a later unrelated `record(` cannot
        // satisfy the check: `\n        }` at the arm's own indent closes it.
        let body = after
            .split_once("\n        }")
            .unwrap_or_else(|| panic!("{arm} is a closed block"))
            .0;
        assert!(
            body.contains("::record("),
            "the arm after {arm:?} returns without recording the file it refused, so the \
             policy section cannot tell it from a file that does not exist:\n{body}"
        );
    }
}
