//! Issue #2279 (C10, D9): `amplihack doctor` checks every settings file that
//! carries amplihack's hooks, one line per file.
//!
//! API this file pins (in `doctor::hook_drift`):
//! - `struct FileReport { path: PathBuf, state: FileDrift }`
//! - `report(home: &Path, repo_local: Option<&Path>, profiles: Source,
//!    launch_file: Source, target: &HookTarget) -> Report`: home, then
//!   profiles sorted by name, then the launch file, then one `Unlisted` per
//!   source that cannot be located or listed. Read-only.
//! - `render(&Report) -> (bool, String)`: one line per file,
//!   `<path>: clean|absent|unparseable (r)|refused (r); <fix>|drift missing=[..] differing=[..] extra=[..]`,
//!   and `<source>: cannot list (r)` per unlisted source. Fails when home is
//!   not clean (an absent home beside a clean repo-local file passes, #1088),
//!   when any present profile or launch file is not clean, or when a source
//!   is unlisted. Absent profile and launch files pass.
//!
//! Every file lives in a temp directory.

#![cfg(unix)]

use super::hook_drift::{FileReport, Source, render, report};
use crate::commands::install::hook_block::{self, FileDrift, HookTarget};
use serde_json::json;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const SECRET: &str = "sk-test-do-not-print";

fn target() -> HookTarget {
    HookTarget {
        bin: PathBuf::from("/opt/amplihack-test/bin/amplihack-hooks"),
        limit_resume: true,
    }
}

fn private(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn full() -> Vec<u8> {
    let mut s = json!({"env": {"SENTINEL_SECRET": SECRET}});
    hook_block::merge(&mut s, &target());
    serde_json::to_vec_pretty(&s).unwrap()
}

fn stripped() -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({"env": {"SENTINEL_SECRET": SECRET}, "hooks": {}})).unwrap()
}

struct Layout {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    launch: PathBuf,
}

fn layout() -> Layout {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("profiles");
    let launch = tmp
        .path()
        .join("amplihack/runtime/launch-hooks-settings.json");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(&root).unwrap();
    Layout {
        _tmp: tmp,
        home,
        root,
        launch,
    }
}

/// `report` with no repo-local file.
fn check(home: &Path, profiles: Source<'_>, launch: Source<'_>) -> super::hook_drift::Report {
    report(home, None, profiles, launch, &target())
}

fn paths(reports: &[FileReport]) -> Vec<PathBuf> {
    reports.iter().map(|r| r.path.clone()).collect()
}

#[test]
fn report_orders_home_then_profiles_by_name_then_launch_file() {
    let l = layout();
    private(&l.home.join(".claude/settings.json"), &full());
    private(&l.root.join("zeta/settings.json"), &full());
    private(&l.root.join("alpha/settings.json"), &full());
    private(&l.launch, &full());
    let r = check(&l.home, Source::At(&l.root), Source::At(&l.launch));
    assert_eq!(
        paths(&r.files),
        vec![
            l.home.join(".claude/settings.json"),
            l.root.join("alpha/settings.json"),
            l.root.join("zeta/settings.json"),
            l.launch.clone(),
        ]
    );
    assert!(r.files.iter().all(|f| f.state == FileDrift::Clean));
    let (ok, text) = render(&r);
    assert!(ok, "{text}");
    assert_eq!(text.lines().count(), 4, "{text}");
}

#[test]
fn a_stripped_profile_fails_and_names_its_events_without_content() {
    let l = layout();
    private(&l.home.join(".claude/settings.json"), &full());
    private(&l.root.join("work/settings.json"), &stripped());
    let r = check(&l.home, Source::At(&l.root), Source::At(&l.launch));
    let (ok, text) = render(&r);
    assert!(!ok, "{text}");
    let line = text
        .lines()
        .find(|line| line.contains("work/settings.json"))
        .unwrap_or_else(|| panic!("no line for the profile: {text}"));
    assert!(line.contains(": drift missing=["), "{line}");
    for event in ["SessionStart", "PreToolUse", "StopFailure"] {
        assert!(line.contains(event), "{event} not named: {line}");
    }
    assert!(!text.contains('{'), "{text}");
    assert!(!text.contains(SECRET), "{text}");
}

#[test]
fn a_stripped_home_fails() {
    let l = layout();
    private(&l.home.join(".claude/settings.json"), &stripped());
    let (ok, text) = render(&check(&l.home, Source::At(&l.root), Source::At(&l.launch)));
    assert!(!ok, "{text}");
}

#[test]
fn an_absent_home_fails_and_absent_profile_and_launch_files_pass() {
    let l = layout();
    fs::create_dir_all(l.root.join("bare")).unwrap();
    let r = check(&l.home, Source::At(&l.root), Source::At(&l.launch));
    assert_eq!(r.files[0].state, FileDrift::Absent);
    let (ok, text) = render(&r);
    assert!(!ok, "an absent home file is not clean: {text}");
    assert!(text.contains(": absent"), "{text}");

    private(&l.home.join(".claude/settings.json"), &full());
    let (ok, text) = render(&check(&l.home, Source::At(&l.root), Source::At(&l.launch)));
    assert!(ok, "absent profile and launch files pass: {text}");
}

#[test]
fn an_unparseable_profile_fails_with_a_reason_and_no_content() {
    let l = layout();
    private(&l.home.join(".claude/settings.json"), &full());
    private(
        &l.root.join("work/settings.json"),
        format!("{{\"k\":\"{SECRET}\", oops").as_bytes(),
    );
    let (ok, text) = render(&check(&l.home, Source::At(&l.root), Source::At(&l.launch)));
    assert!(!ok);
    assert!(text.contains(": unparseable ("), "{text}");
    assert!(!text.contains(SECRET), "{text}");
}

#[test]
fn a_drifting_launch_file_fails() {
    let l = layout();
    private(&l.home.join(".claude/settings.json"), &full());
    private(&l.launch, &stripped());
    let (ok, text) = render(&check(&l.home, Source::At(&l.root), Source::At(&l.launch)));
    assert!(!ok, "{text}");
}

#[test]
fn report_is_read_only() {
    let l = layout();
    let files = [
        l.home.join(".claude/settings.json"),
        l.root.join("work/settings.json"),
    ];
    private(&files[0], &stripped());
    private(&files[1], &stripped());
    let before: Vec<(Vec<u8>, i64, i64)> = files
        .iter()
        .map(|p| {
            let m = fs::metadata(p).unwrap();
            (fs::read(p).unwrap(), m.mtime(), m.mtime_nsec())
        })
        .collect();
    check(&l.home, Source::At(&l.root), Source::At(&l.launch));
    let after: Vec<(Vec<u8>, i64, i64)> = files
        .iter()
        .map(|p| {
            let m = fs::metadata(p).unwrap();
            (fs::read(p).unwrap(), m.mtime(), m.mtime_nsec())
        })
        .collect();
    assert_eq!(after, before);
    assert!(!l.launch.exists());
}

#[test]
fn an_unreadable_profile_root_is_a_failing_row_and_nothing_is_written() {
    let l = layout();
    let home_file = l.home.join(".claude/settings.json");
    private(&home_file, &full());
    private(&l.root.join("work/settings.json"), &stripped());
    fs::set_permissions(&l.root, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&l.root).is_ok();
    let before = fs::read(&home_file).unwrap();
    let r = check(&l.home, Source::At(&l.root), Source::At(&l.launch));
    fs::set_permissions(&l.root, fs::Permissions::from_mode(0o700)).unwrap();
    if readable {
        // Running as root: mode 0000 does not stop the listing.
        return;
    }
    let (ok, text) = render(&r);
    assert!(!ok, "{text}");
    assert!(
        text.lines()
            .any(|line| line.starts_with("profiles: cannot list (")),
        "{text}"
    );
    assert!(!text.contains(SECRET), "{text}");
    assert_eq!(fs::read(&home_file).unwrap(), before);
    assert!(!l.launch.exists());
}

#[test]
fn an_unknown_source_is_a_failing_row() {
    let l = layout();
    private(&l.home.join(".claude/settings.json"), &full());
    let (ok, text) = render(&check(
        &l.home,
        Source::Unknown("not configured"),
        Source::Unknown("amplihack home unknown or not absolute"),
    ));
    assert!(!ok, "{text}");
    assert!(
        text.contains("profiles: cannot list (not configured)"),
        "{text}"
    );
    assert!(text.contains("launch file: cannot list ("), "{text}");
}

/// #1088, narrowed by #2279: an absent home file beside a clean repo-local
/// file passes; a stale repo-local file does not.
#[test]
fn test_check_hooks_installed_passes_with_repo_local_hooks_only() {
    let l = layout();
    let repo = l.home.parent().unwrap().join("repo/.claude/settings.json");
    private(&repo, &full());
    let r = report(
        &l.home,
        Some(&repo),
        Source::At(&l.root),
        Source::At(&l.launch),
        &target(),
    );
    let (ok, text) = render(&r);
    assert!(ok, "a clean repo-local install passes: {text}");
    assert!(
        text.contains(": absent; repo-local install clean"),
        "{text}"
    );
    assert_eq!(r.files.len(), 2, "the repo-local file has no row: {text}");

    private(&repo, &stripped());
    let r = report(
        &l.home,
        Some(&repo),
        Source::At(&l.root),
        Source::At(&l.launch),
        &target(),
    );
    let (ok, text) = render(&r);
    assert!(!ok, "a stale repo-local file does not pass: {text}");

    // A present home file is judged on its own.
    private(&repo, &full());
    private(&l.home.join(".claude/settings.json"), &stripped());
    let (ok, text) = render(&report(
        &l.home,
        Some(&repo),
        Source::At(&l.root),
        Source::At(&l.launch),
        &target(),
    ));
    assert!(!ok, "{text}");
}

// ── B4: a linked home file is one failing row with the fix ────────────────

fn assert_refused_row_with_fix(l: &Layout, target_file: &Path, before: &[u8]) {
    let (ok, text) = render(&check(&l.home, Source::At(&l.root), Source::At(&l.launch)));
    assert!(!ok, "{text}");
    let row = text
        .lines()
        .find(|line| line.contains("settings.json: refused ("))
        .unwrap_or_else(|| panic!("no refused row: {text}"));
    assert!(
        row.ends_with("replace it with a regular file you own"),
        "{row}"
    );
    assert!(!text.contains(SECRET), "{text}");
    assert_eq!(fs::read(target_file).unwrap(), before);
}

#[test]
fn a_symlinked_home_file_is_a_failing_row_with_the_fix_and_is_unchanged() {
    let l = layout();
    let real = l.home.join("real-settings.json");
    private(&real, &stripped());
    std::os::unix::fs::symlink(&real, l.home.join(".claude/settings.json")).unwrap();
    assert_refused_row_with_fix(&l, &real, &stripped());
    assert!(
        fs::symlink_metadata(l.home.join(".claude/settings.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn a_hard_linked_home_file_is_a_failing_row_with_the_fix_and_is_unchanged() {
    let l = layout();
    let other = l.home.join("other-name.json");
    private(&other, &stripped());
    fs::hard_link(&other, l.home.join(".claude/settings.json")).unwrap();
    assert_refused_row_with_fix(&l, &other, &stripped());
    assert_eq!(fs::metadata(&other).unwrap().nlink(), 2);
}
