//! `codex::state` (issue #2005): `<rotate dir>/codex-state.json`.
//!
//! Contract:
//! - `CodexState { version: u32, journal: Option<CodexJournal>,
//!   last_swap_at: Option<i64>, resumes: Vec<ResumeRecord> }`;
//!   `CodexState::default().version == 1`.
//! - `CodexJournal { from, to: String, link, old_target, new_target:
//!   PathBuf, from_ino, to_ino: u64, phase: CodexPhase }`;
//!   `CodexPhase::{Prepared, Linked}` serialised `prepared` / `linked`.
//! - `ResumeRecord { session, from, to: String, at: i64 }`.
//! - `load(dir) -> io::Result<CodexState>`: missing is `Default`; a file
//!   that does not parse, has another version, is not a regular file, is
//!   not ours or is wider than 0600 is `Err(InvalidData)`.
//! - `save(dir, &state, now) -> io::Result<()>` drops resumes older than
//!   24 hours and writes atomically with mode 0600.
//! - `CodexState::resumes_in_window(session, now) -> u32`.

use super::state::{CodexJournal, CodexPhase, CodexState, ResumeRecord, load, save};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const NOW: i64 = 1_794_200_000;

fn dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}

fn file(d: &Path) -> PathBuf {
    d.join("codex-state.json")
}

fn rec(session: &str, at: i64) -> ResumeRecord {
    ResumeRecord {
        session: session.into(),
        at,
        from: "a".into(),
        to: "b".into(),
    }
}

#[test]
fn a_missing_file_is_the_default_state() {
    let d = dir();
    let s = load(d.path()).unwrap();
    assert_eq!(s, CodexState::default());
    assert_eq!(s.version, 1);
    assert!(s.journal.is_none() && s.last_swap_at.is_none() && s.resumes.is_empty());
}

#[test]
fn a_state_round_trips_with_mode_0600() {
    let d = dir();
    let s = CodexState {
        journal: Some(CodexJournal {
            from: "a".into(),
            to: "b".into(),
            link: "/h/.codex".into(),
            old_target: "/r/a".into(),
            new_target: "/r/b".into(),
            from_ino: 11,
            to_ino: 22,
            phase: CodexPhase::Prepared,
        }),
        last_swap_at: Some(NOW - 5),
        resumes: vec![rec("s1", NOW - 10)],
        ..CodexState::default()
    };
    save(d.path(), &s, NOW).unwrap();
    assert_eq!(load(d.path()).unwrap(), s);
    let m = fs::symlink_metadata(file(d.path())).unwrap();
    assert!(m.file_type().is_file());
    assert_eq!(m.permissions().mode() & 0o777, 0o600);
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(file(d.path())).unwrap()).unwrap();
    assert_eq!(raw["version"], 1);
    assert_eq!(raw["journal"]["phase"], "prepared");
}

#[test]
fn save_drops_resume_records_older_than_a_day() {
    let d = dir();
    let s = CodexState {
        resumes: vec![
            rec("s", NOW - 86_401),
            rec("s", NOW - 86_399),
            rec("t", NOW),
        ],
        ..CodexState::default()
    };
    save(d.path(), &s, NOW).unwrap();
    let back = load(d.path()).unwrap();
    assert_eq!(back.resumes, vec![rec("s", NOW - 86_399), rec("t", NOW)]);
}

#[test]
fn resumes_in_window_counts_one_session_in_the_last_day() {
    let s = CodexState {
        resumes: vec![
            rec("s", NOW - 100),
            rec("s", NOW - 86_400),
            rec("s", NOW - 90_000),
            rec("t", NOW),
        ],
        ..CodexState::default()
    };
    assert_eq!(s.resumes_in_window("s", NOW), 1);
    assert_eq!(s.resumes_in_window("t", NOW), 1);
    assert_eq!(s.resumes_in_window("u", NOW), 0);
}

fn assert_invalid(d: &Path) {
    match load(d) {
        Err(e) => assert_eq!(e.kind(), ErrorKind::InvalidData, "{e}"),
        Ok(s) => panic!("loaded {s:?}"),
    }
}

#[test]
fn an_unparsable_or_unknown_version_file_is_refused_and_kept() {
    for body in [
        "{nope",
        r#"{"version":2,"resumes":[]}"#,
        r#"{"resumes":[]}"#,
    ] {
        let d = dir();
        fs::write(file(d.path()), body).unwrap();
        fs::set_permissions(file(d.path()), fs::Permissions::from_mode(0o600)).unwrap();
        assert_invalid(d.path());
        assert_eq!(fs::read_to_string(file(d.path())).unwrap(), body);
    }
}

#[test]
fn a_wide_mode_a_symlink_or_a_directory_is_refused() {
    let d = dir();
    fs::write(file(d.path()), r#"{"version":1,"resumes":[]}"#).unwrap();
    fs::set_permissions(file(d.path()), fs::Permissions::from_mode(0o640)).unwrap();
    assert_invalid(d.path());

    let d = dir();
    let real = d.path().join("real.json");
    fs::write(&real, r#"{"version":1,"resumes":[]}"#).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&real, file(d.path())).unwrap();
    assert_invalid(d.path());

    let d = dir();
    fs::create_dir(file(d.path())).unwrap();
    assert_invalid(d.path());
}

#[test]
fn save_never_writes_through_a_symlink() {
    let d = dir();
    let victim = d.path().join("victim");
    fs::write(&victim, "keep").unwrap();
    symlink(&victim, file(d.path())).unwrap();
    let _ = save(d.path(), &CodexState::default(), NOW);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
}
