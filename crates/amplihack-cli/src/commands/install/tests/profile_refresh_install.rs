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

    let results = refresh_profile_settings(&hooks, &|_name: &str, s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([]);
    })
    .results;
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
    let results = refresh_profile_settings(&hooks, &|_name: &str, s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([]);
    })
    .results;
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
    let results = refresh_profile_settings(missing, &|_name: &str, s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([]);
    })
    .results;
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

// --- a profile root that cannot be read (issue #2589, R6) ------------------
//
// `refresh_profile_settings` returns an empty list both when there are no
// profiles and when the root could not be read at all, so the install's
// summary line said the deny rules were written "in every managed profile
// that has one" after a run that read no profile at all. The two cases are
// now told apart and the failure travels back to the caller, which records
// it through the same `policy::claude::record` path the per-profile and
// home-settings refusals already use.
//
// Expected API:
//
// ```ignore
// pub(crate) struct ProfileRefresh {
//     pub results: Vec<(String, Refresh)>,
//     /// Why no profile could be read: the root could not be resolved, or
//     /// listing it failed. `None` when the root was read, including when
//     /// it holds no profiles.
//     pub root_failure: Option<String>,
// }
// pub(crate) fn refresh_profile_settings(..) -> ProfileRefresh;
//
// // in install/settings.rs
// pub(super) const PROFILE_ROOT_LABEL: &str = "managed profiles";
// ```

use super::super::settings::PROFILE_ROOT_LABEL;
use crate::policy;
use crate::test_support::tempdir_outside_framework_roots;

/// Empties the Claude policy record left by any earlier call on this thread.
fn clear_policy_record() {
    let _ = policy::claude::take_record();
}

/// A temp HOME with a hooks stub, a real profile directory and a symlink
/// standing where the profile root belongs. Returns the home, the symlinked
/// root and the hooks binary.
fn home_with_symlinked_root() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let real = home.join("real-profiles");
    profile_settings(&real);
    let root = home.join("profiles");
    std::os::unix::fs::symlink(&real, &root).unwrap();
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    (d, root, hooks)
}

/// A symlinked root is a failure, not an empty set of profiles: the refusal
/// reaches the record, so the Claude line stops claiming a complete write.
#[test]
fn a_symlinked_profile_root_is_recorded_rather_than_counted_as_a_success() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let (d, root, hooks) = home_with_symlinked_root();
    let home = d.path().join("home");
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);

    // A home settings file that did take the entries, so the record holds
    // one success beside the root failure and the outcome is `Partial`
    // rather than `AllRefused`.
    policy::claude::record("home settings", &policy::claude::DenyOutcome::Written);
    refresh_profile_hooks(&hooks);

    let (outcome, warnings) = policy::claude::take_record();
    assert_eq!(
        outcome,
        policy::HarnessWrite::Partial,
        "the home file took the entries and the profile root was never read: {warnings:?}"
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains(PROFILE_ROOT_LABEL),
        "the warning names the profile root, not one profile: {:?}",
        warnings[0]
    );
    assert!(
        !warnings[0].contains(&root.display().to_string())
            && !warnings[0].contains(&d.path().display().to_string()),
        "the reason is a short phrase chosen from the condition, never a path \
         taken from the error's own text: {:?}",
        warnings[0]
    );
}

/// The caller is told, so the record is not the only channel and the install
/// can print its own line about the root.
#[test]
fn a_symlinked_profile_root_is_returned_to_the_caller_as_a_root_failure() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let (d, root, hooks) = home_with_symlinked_root();
    let home = d.path().join("home");
    let _env = Env::set(&[(ROOT_ENV, Some(&root)), ("HOME", Some(&home))]);

    let refresh = refresh_profile_settings(&hooks, &|_name: &str, s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([]);
    });
    assert!(
        refresh.results.is_empty(),
        "no profile was read: {:?}",
        refresh.results
    );
    let reason = refresh
        .root_failure
        .expect("a root that cannot be listed is a failure, not an empty set of profiles");
    assert!(!reason.is_empty(), "the reason says what went wrong");
    assert!(
        !reason.contains(&root.display().to_string()),
        "the reason carries no path: {reason}"
    );
    assert!(!reason.contains('\n'), "one line: {reason}");
}

/// A root that simply is not there stays "nothing to write into": no record,
/// no warning, and no line claiming a write either.
#[test]
fn an_absent_profile_root_is_not_a_failure_and_warns_about_nothing() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    clear_policy_record();
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let root = home.join("profiles");
    assert!(!root.exists(), "the fixture creates no root");
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);

    let refresh = refresh_profile_settings(&hooks, &|_name: &str, s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([]);
    });
    assert!(refresh.results.is_empty(), "{:?}", refresh.results);
    assert_eq!(
        refresh.root_failure, None,
        "an absent root is nothing to write into, not a refusal"
    );

    refresh_profile_hooks(&hooks);
    let (outcome, warnings) = policy::claude::take_record();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        outcome,
        policy::HarnessWrite::NothingToWriteInto,
        "nothing was met, so nothing is claimed and nothing failed"
    );
    assert!(!root.exists(), "and no root is created");
}

/// A root that holds profiles is still read: the new failure channel does
/// not fire on an ordinary run, which would train a user to ignore it.
#[test]
fn a_readable_profile_root_reports_no_root_failure() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let d = tempdir_outside_framework_roots();
    let home = d.path().join("home");
    let root = home.join("profiles");
    profile_settings(&root);
    let hooks = hooks_file(&home.join(".local/bin/amplihack-hooks"));
    let _env = Env::set(&[
        (ROOT_ENV, Some(&root)),
        (PROFILE_REFRESH_TEST_OPT_IN, Some(Path::new("1"))),
        ("HOME", Some(&home)),
    ]);

    let refresh = refresh_profile_settings(&hooks, &|_name: &str, s: &mut Value| {
        s["hooks"]["StopFailure"] = serde_json::json!([]);
    });
    assert_eq!(refresh.root_failure, None, "the root was read");
    assert_eq!(refresh.results.len(), 1, "{:?}", refresh.results);
}
