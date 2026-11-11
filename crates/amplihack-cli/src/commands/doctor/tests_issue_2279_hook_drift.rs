//! Issue #2279 (C10, D9): `amplihack doctor` checks every settings file that
//! carries amplihack's hooks, one line per file.
//!
//! API this file pins (in `doctor::hook_drift`):
//! - `struct FileReport { path: PathBuf, state: FileDrift }`
//! - `report(home: &Path, profile_root: Option<&Path>, launch_file: Option<&Path>,
//!    target: &HookTarget) -> Vec<FileReport>`: home, then profiles sorted by
//!   name, then the launch file. Read-only.
//! - `render(&[FileReport]) -> (bool, String)`: one line per file,
//!   `<path>: clean|absent|unparseable (r)|refused (r)|drift missing=[..] differing=[..] extra=[..]`.
//!   Fails when home is not clean, or any present profile or launch file is
//!   not clean. Absent profile and launch files pass.
//!
//! Every file lives in a temp directory.

#![cfg(unix)]

use super::hook_drift::{FileReport, render, report};
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
    let r = report(&l.home, Some(&l.root), Some(&l.launch), &target());
    assert_eq!(
        paths(&r),
        vec![
            l.home.join(".claude/settings.json"),
            l.root.join("alpha/settings.json"),
            l.root.join("zeta/settings.json"),
            l.launch.clone(),
        ]
    );
    assert!(r.iter().all(|f| f.state == FileDrift::Clean));
    let (ok, text) = render(&r);
    assert!(ok, "{text}");
    assert_eq!(text.lines().count(), 4, "{text}");
}

#[test]
fn a_stripped_profile_fails_and_names_its_events_without_content() {
    let l = layout();
    private(&l.home.join(".claude/settings.json"), &full());
    private(&l.root.join("work/settings.json"), &stripped());
    let r = report(&l.home, Some(&l.root), None, &target());
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
    let (ok, text) = render(&report(&l.home, Some(&l.root), None, &target()));
    assert!(!ok, "{text}");
}

#[test]
fn an_absent_home_fails_and_absent_profile_and_launch_files_pass() {
    let l = layout();
    fs::create_dir_all(l.root.join("bare")).unwrap();
    let r = report(&l.home, Some(&l.root), Some(&l.launch), &target());
    assert_eq!(r[0].state, FileDrift::Absent);
    let (ok, text) = render(&r);
    assert!(!ok, "an absent home file is not clean: {text}");
    assert!(text.contains(": absent"), "{text}");

    private(&l.home.join(".claude/settings.json"), &full());
    let (ok, text) = render(&report(&l.home, Some(&l.root), Some(&l.launch), &target()));
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
    let (ok, text) = render(&report(&l.home, Some(&l.root), None, &target()));
    assert!(!ok);
    assert!(text.contains(": unparseable ("), "{text}");
    assert!(!text.contains(SECRET), "{text}");
}

#[test]
fn a_drifting_launch_file_fails() {
    let l = layout();
    private(&l.home.join(".claude/settings.json"), &full());
    private(&l.launch, &stripped());
    let (ok, text) = render(&report(&l.home, None, Some(&l.launch), &target()));
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
    report(&l.home, Some(&l.root), Some(&l.launch), &target());
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
