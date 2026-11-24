//! Issue #3172: the resume log never fails silently, and its error field
//! never carries message text.
//!
//! - `reduce_error` is the only route from a payload's `error` value to the
//!   log: lowercase ASCII, every other character `_`, at most 32 characters,
//!   and an absent value is the empty string.
//! - `append` returns why it wrote nothing, with a fixed token, and does not
//!   hang on a FIFO standing in for the log or the lock.
//! - `mark_failure` leaves the one fixed-name marker `log-write-failed`, but
//!   only in a private resume folder.
//!
//! No test here sleeps or sets a timeout.

#![cfg(unix)]

use amplihack_hooks::limit_resume::Outcome;
use amplihack_hooks::limit_resume::log::{
    LogEntry, LogWriteFailure, append, format_line, mark_failure, reduce_error,
};
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const NOW: i64 = 1_792_592_000;
const MARKER: &str = "log-write-failed";

fn dir_with_mode(mode: u32) -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("resume");
    fs::create_dir(&d).unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(mode)).unwrap();
    (t, d)
}

fn mkfifo(p: &Path) {
    let c = CString::new(p.as_os_str().as_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

fn is_reduced(s: &str) -> bool {
    s.len() <= 32 && s.bytes().all(|b| b == b'_' || b.is_ascii_lowercase())
}

fn entry(error: &str) -> LogEntry {
    LogEntry {
        session: "3f9a1c2b7e04".into(),
        event: "-".into(),
        outcome: Outcome::NotLimit,
        live: None,
        cycle: None,
        swap_seq: None,
        swap_live: None,
        pane: None,
        keys_sent: None,
        resumes: None,
        detail: None,
        error: error.into(),
    }
}

// ── reduce_error ────────────────────────────────────────────────────────────

#[test]
fn reduce_error_keeps_a_clean_value() {
    assert_eq!(reduce_error(Some("rate_limit")), "rate_limit");
    assert_eq!(reduce_error(Some("server_error")), "server_error");
}

#[test]
fn reduce_error_of_an_absent_value_is_empty() {
    assert_eq!(reduce_error(None), "");
    assert_eq!(reduce_error(Some("")), "");
}

#[test]
fn reduce_error_lowercases_and_replaces_everything_else() {
    assert_eq!(reduce_error(Some("Rate-Limit!")), "rate_limit_");
    assert_eq!(reduce_error(Some("a b\nc\td")), "a_b_c_d");
    assert_eq!(reduce_error(Some("a=b")), "a_b");
    assert_eq!(reduce_error(Some("x1y2")), "x_y_");
}

#[test]
fn reduce_error_hostile_input_always_matches_the_alphabet() {
    let long = "A".repeat(1024 * 1024);
    for raw in [
        "ÅÉÎ",
        "🙂rate_limit",
        "\u{0}\u{1b}[31mred",
        "line one\nline two: message text",
        "../../etc/passwd",
        "rate_limit error=disabled outcome=resumed-keys",
        long.as_str(),
    ] {
        let out = reduce_error(Some(raw));
        assert!(is_reduced(&out), "{raw:.20?} gave {out:?}");
    }
}

#[test]
fn reduce_error_is_capped_at_32_characters() {
    assert_eq!(reduce_error(Some(&"a".repeat(32))).len(), 32);
    assert_eq!(reduce_error(Some(&"a".repeat(33))).len(), 32);
    assert_eq!(reduce_error(Some(&"é".repeat(40))).len(), 32);
}

// ── format_line ─────────────────────────────────────────────────────────────

#[test]
fn every_line_ends_with_the_error_field() {
    for e in ["", "rate_limit", "server_error"] {
        let line = format_line(NOW, &entry(e));
        assert!(line.ends_with(&format!(" error={e}\n")), "{line:?}");
        assert_eq!(line.matches('\n').count(), 1);
    }
}

#[test]
fn the_error_field_comes_after_the_optional_fields() {
    let mut e = entry("rate_limit");
    e.outcome = Outcome::ResumedKeys;
    e.pane = Some("0c4e61a8d2f3".into());
    e.keys_sent = Some(4);
    e.detail = Some("x".into());
    let line = format_line(NOW, &e);
    assert!(line.ends_with(" detail=x error=rate_limit\n"), "{line:?}");
}

#[test]
fn a_gate_line_has_dashes_for_what_does_not_exist() {
    let line = format_line(NOW, &entry("server_error"));
    assert!(line.contains(" event=- "), "{line:?}");
    assert!(line.contains("outcome=not-limit"), "{line:?}");
}

// ── append: every failure is reported ───────────────────────────────────────

#[test]
fn append_to_a_private_directory_succeeds() {
    let (_t, d) = dir_with_mode(0o700);
    append(&d, "one\n").unwrap();
    assert_eq!(fs::read_to_string(d.join("resume.log")).unwrap(), "one\n");
}

#[test]
fn append_reports_a_missing_directory_and_creates_nothing() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("absent");
    assert_eq!(append(&d, "x\n"), Err(LogWriteFailure::DirMissing));
    assert!(!d.exists());
}

#[test]
fn append_reports_an_unprivate_directory_and_leaves_no_marker() {
    let (_t, d) = dir_with_mode(0o755);
    assert_eq!(append(&d, "x\n"), Err(LogWriteFailure::DirUnprivate));
    assert!(!d.join("resume.log").exists());
    assert!(!d.join(MARKER).exists());
    assert!(!d.join("lock").exists(), "nothing is created in it");
}

#[test]
fn append_reports_a_symlinked_log() {
    let (t, d) = dir_with_mode(0o700);
    let target = t.path().join("target");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, d.join("resume.log")).unwrap();
    assert_eq!(append(&d, "x\n"), Err(LogWriteFailure::OpenFailed));
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}

#[test]
fn append_does_not_hang_on_a_fifo_standing_in_for_the_log() {
    let (_t, d) = dir_with_mode(0o700);
    mkfifo(&d.join("resume.log"));
    // Opening a FIFO for writing blocks until a reader exists; the log must
    // refuse it instead.
    assert_eq!(append(&d, "x\n"), Err(LogWriteFailure::OpenFailed));
}

#[test]
fn append_does_not_hang_on_a_fifo_standing_in_for_the_lock() {
    let (_t, d) = dir_with_mode(0o700);
    mkfifo(&d.join("lock"));
    let r = append(&d, "x\n");
    assert!(
        matches!(
            r,
            Err(LogWriteFailure::OpenFailed | LogWriteFailure::LockFailed)
        ),
        "{r:?}"
    );
    assert!(!d.join("resume.log").exists());
}

#[test]
fn append_creates_a_log_owned_by_us_with_mode_0600() {
    let (_t, d) = dir_with_mode(0o700);
    append(&d, "x\n").unwrap();
    let meta = fs::metadata(d.join("resume.log")).unwrap();
    // SAFETY: geteuid has no preconditions.
    assert_eq!(meta.uid(), unsafe { libc::geteuid() });
    assert_eq!(meta.mode() & 0o7777, 0o600);
}

// ── mark_failure ────────────────────────────────────────────────────────────

#[test]
fn mark_failure_writes_one_fixed_name_private_marker() {
    let (_t, d) = dir_with_mode(0o700);
    mark_failure(&d, NOW, Outcome::ResumedKeys, LogWriteFailure::LockFailed);
    let marker = d.join(MARKER);
    let text = fs::read_to_string(&marker).unwrap();
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert!(text.contains("resumed-keys"), "{text:?}");
    assert!(text.contains("lock_failed"), "{text:?}");
    assert_eq!(fs::metadata(&marker).unwrap().mode() & 0o7777, 0o600);
}

#[test]
fn mark_failure_overwrites_and_does_not_grow() {
    let (_t, d) = dir_with_mode(0o700);
    for _ in 0..5 {
        mark_failure(&d, NOW, Outcome::Duplicate, LogWriteFailure::OpenFailed);
    }
    mark_failure(&d, NOW + 1, Outcome::NotLimit, LogWriteFailure::WriteFailed);
    let text = fs::read_to_string(d.join(MARKER)).unwrap();
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert!(
        text.contains("not-limit") && text.contains("write_failed"),
        "{text:?}"
    );
    assert!(!text.contains("duplicate"));
}

#[test]
fn mark_failure_writes_nothing_into_an_unprivate_directory() {
    let (_t, d) = dir_with_mode(0o755);
    mark_failure(&d, NOW, Outcome::NotLimit, LogWriteFailure::DirUnprivate);
    assert!(!d.join(MARKER).exists());
}

#[test]
fn mark_failure_into_a_missing_directory_creates_nothing_and_does_not_panic() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("absent");
    mark_failure(&d, NOW, Outcome::NotLimit, LogWriteFailure::DirMissing);
    assert!(!d.exists());
}

#[test]
fn mark_failure_does_not_write_through_a_symlinked_marker() {
    let (t, d) = dir_with_mode(0o700);
    let target = t.path().join("target");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, d.join(MARKER)).unwrap();
    mark_failure(&d, NOW, Outcome::NotLimit, LogWriteFailure::OpenFailed);
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}
