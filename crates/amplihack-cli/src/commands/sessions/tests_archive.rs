//! `sessions::archive` (issue #1746): the pure parts of an archive run.
//! The whole run (verify, link, manifest, removal) is covered end to end by
//! `tests/issue_1746_sessions_*.rs`.
//!
//! Contract:
//! - `cutoff(now: SystemTime, days: u32) -> Option<SystemTime>`: `now −
//!   days × 86400`, `None` on overflow; `is_old(newest, cutoff)` is
//!   `newest <= cutoff` and false for `None` (nothing is old).
//! - `archive_names(id: &str, ts: &str) -> Vec<String>`: `<id>.tar.gz`,
//!   `<id>.<ts>.tar.gz`, then `<id>.<ts>-1.tar.gz` to `-9`: eleven names.
//! - `Report { archived: u64, bytes: u64, live_kept: u64, failed: u64 }`
//!   and `summary(&Report, days: u32, apply: bool) -> String`.
//! - `ArchiveError::{LockHeld(PathBuf), DestOverlapsProjects(PathBuf,
//!   PathBuf), NotPrivate(PathBuf), Setup(PathBuf, io::ErrorKind),
//!   LiveUnreadable(PathBuf, io::ErrorKind)}` with one `Display` each.

use super::archive::{ArchiveError, Report, archive_names, cutoff, is_old, summary};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn cutoff_is_days_before_now() {
    let now = UNIX_EPOCH + Duration::from_secs(10 * 86_400);
    assert_eq!(
        cutoff(now, 3),
        Some(UNIX_EPOCH + Duration::from_secs(7 * 86_400))
    );
}

#[test]
fn cutoff_overflow_means_nothing_is_old() {
    let c = cutoff(UNIX_EPOCH, u32::MAX);
    // Either representable or None; if None, nothing counts as old.
    if c.is_none() {
        assert!(!is_old(UNIX_EPOCH, c));
    }
    assert!(!is_old(SystemTime::now(), None));
}

#[test]
fn at_the_cutoff_is_old_and_one_second_after_is_not() {
    let now = UNIX_EPOCH + Duration::from_secs(100 * 86_400);
    let c = cutoff(now, 3);
    let edge = c.unwrap();
    assert!(is_old(edge, c));
    assert!(is_old(edge - Duration::from_secs(1), c));
    assert!(!is_old(edge + Duration::from_secs(1), c));
}

#[test]
fn eleven_names_and_never_more() {
    let n = archive_names("abc", "20261029T120000Z");
    assert_eq!(n.len(), 11);
    assert_eq!(n[0], "abc.tar.gz");
    assert_eq!(n[1], "abc.20261029T120000Z.tar.gz");
    assert_eq!(n[2], "abc.20261029T120000Z-1.tar.gz");
    assert_eq!(n[10], "abc.20261029T120000Z-9.tar.gz");
}

fn report(archived: u64, bytes: u64, live_kept: u64, failed: u64) -> Report {
    Report {
        archived,
        bytes,
        live_kept,
        failed,
        stopped: false,
    }
}

#[test]
fn apply_summary() {
    assert_eq!(
        summary(&report(28_013, 8_611_000_000, 61, 0), 3, true),
        "archived 28,013 sessions, 8.02 GiB (older than 3 days; 61 live sessions kept; 0 failed)"
    );
    assert_eq!(
        summary(&report(1, 0, 0, 2), 7, true),
        "archived 1 sessions, 0.00 GiB (older than 7 days; 0 live sessions kept; 2 failed)"
    );
}

#[test]
fn dry_run_summary() {
    assert_eq!(
        summary(&report(28_013, 8_611_000_000, 61, 0), 3, false),
        "would archive 28,013 sessions, 8.02 GiB (older than 3 days; 61 live sessions kept); dry run, nothing changed (use --apply)"
    );
    assert_eq!(
        summary(&report(0, 0, 0, 0), 3, false),
        "would archive 0 sessions, 0.00 GiB (older than 3 days; 0 live sessions kept); dry run, nothing changed (use --apply)"
    );
}

#[test]
fn error_lines() {
    let p = PathBuf::from("/d");
    assert_eq!(
        ArchiveError::LockHeld(PathBuf::from("/d/.archive.lock")).to_string(),
        "another sessions archive run is in progress (lock: /d/.archive.lock)"
    );
    assert_eq!(
        ArchiveError::DestOverlapsProjects(p.clone(), PathBuf::from("/h/.claude/projects"))
            .to_string(),
        "destination /d overlaps /h/.claude/projects; nothing changed"
    );
    assert_eq!(
        ArchiveError::NotPrivate(p.clone()).to_string(),
        "destination /d is not private (must be your own directory, not writable by group or others)"
    );
    assert_eq!(
        ArchiveError::Setup(p.clone(), ErrorKind::NotFound).to_string(),
        "cannot prepare /d (not found)"
    );
    assert_eq!(
        ArchiveError::LiveUnreadable(
            PathBuf::from("/p/work/sessions"),
            ErrorKind::PermissionDenied
        )
        .to_string(),
        "cannot read live sessions in /p/work/sessions (permission denied); nothing archived"
    );
}

#[test]
fn error_lines_escape_control_characters() {
    let e = ArchiveError::NotPrivate(PathBuf::from("/d\n\u{1b}[31m"));
    assert!(!e.to_string().chars().any(char::is_control), "{e}");
}
