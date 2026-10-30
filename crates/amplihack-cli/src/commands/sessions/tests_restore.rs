//! `sessions::restore` internals (issue #1746): the manifest-line and
//! member rules, and the byte budget that stops extraction. The end-to-end
//! refusals are in `tests/issue_1746_sessions_lock_restore.rs`.

use super::{Extract, RestoreError, RestoreReport, line_ok, member_ok};
use crate::commands::sessions::store::Entry;
use std::path::{Path, PathBuf};
use tar::EntryType;

const ID: &str = "abc";

fn line(project: &str, archive: &str) -> Entry {
    Entry {
        time: "t".into(),
        session_id: ID.into(),
        project: project.into(),
        bytes: 1,
        archive: archive.into(),
        parts: 1,
    }
}

#[test]
fn manifest_line_rules() {
    assert!(line_ok(&line("-p", "-p/abc.tar.gz"), ID));
    assert!(line_ok(&line("-p", "-p/abc.20261029T120000Z-3.tar.gz"), ID));
    for (project, archive) in [
        ("-p", "/-p/abc.tar.gz"),
        ("-p", "-p/../abc.tar.gz"),
        ("-p", "-p/x/abc.tar.gz"),
        ("-p", "-q/abc.tar.gz"),
        ("-p", "-p/abc.zip"),
        ("-p", "-p/.abc.tar.gz.tmp-1"),
        ("..", "../abc.tar.gz"),
        ("", "abc.tar.gz"),
    ] {
        assert!(!line_ok(&line(project, archive), ID), "{project} {archive}");
    }
    assert!(!line_ok(&line("-p", "-p/abc.tar.gz"), "other"));
}

#[test]
fn member_rules() {
    let ok = [
        ("abc.jsonl", EntryType::Regular),
        ("abc", EntryType::Directory),
        ("abc/", EntryType::Directory),
        ("abc/sub/a", EntryType::Regular),
        ("abc/sub", EntryType::Directory),
    ];
    for (p, k) in ok {
        assert!(member_ok(Path::new(p), k, ID), "{p}");
    }
    let bad = [
        ("abc.jsonl", EntryType::Directory),
        ("abc", EntryType::Regular),
        ("abc/x", EntryType::Symlink),
        ("abc/x", EntryType::Link),
        ("abc/x", EntryType::Fifo),
        ("/abc/x", EntryType::Regular),
        ("../abc/x", EntryType::Regular),
        ("abc/../../x", EntryType::Regular),
        ("./abc/x", EntryType::Regular),
        ("abcd/x", EntryType::Regular),
        ("other.jsonl", EntryType::Regular),
    ];
    for (p, k) in bad {
        assert!(!member_ok(Path::new(p), k, ID), "{p} {k:?}");
    }
}

#[test]
fn writing_past_the_manifest_bytes_stops_extraction() {
    let t = tempfile::tempdir().unwrap();
    let mut lines = Vec::new();
    let mut warn = |l: &str| lines.push(l.to_string());
    let archive = PathBuf::from("/d/-p/abc.tar.gz");
    let mut x = Extract {
        archive: &archive,
        id: ID,
        target: t.path(),
        budget: 3,
        restored: 0,
        conflicts: 0,
        cut: Vec::new(),
        warn: &mut warn,
    };
    x.write_file(Path::new("abc.jsonl"), &mut &b"12"[..])
        .unwrap();
    let err = x
        .write_file(Path::new("abc.more"), &mut &b"34"[..])
        .unwrap_err();
    assert!(matches!(err, RestoreError::TooMuchData(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "refused /d/-p/abc.tar.gz: more data than the manifest records"
    );
    assert_eq!(x.restored, 1);
}

#[test]
fn summary_line() {
    let r = |restored, conflicts| RestoreReport {
        restored,
        conflicts,
        target: PathBuf::from("/h/.claude/projects/-p"),
    };
    assert_eq!(
        r(5, 0).summary(),
        "restored 5 files to /h/.claude/projects/-p"
    );
    assert_eq!(
        r(1, 1).summary(),
        "restored 1 files to /h/.claude/projects/-p; 1 conflict kept"
    );
    assert_eq!(
        r(0, 2).summary(),
        "restored 0 files to /h/.claude/projects/-p; 2 conflicts kept"
    );
}
