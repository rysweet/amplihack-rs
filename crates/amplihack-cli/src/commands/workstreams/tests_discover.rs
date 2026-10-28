//! Issue #1710 (D4): finding runs in the recipe log directory and keeping
//! the caller's descendants.
//!
//! Contract (`discover`):
//! - `scan(log_dir) -> Vec<Discovered>`: one entry per session found as a
//!   readable `<session>.run.json` or a `<session>.log`; names containing
//!   `.tmp.` and `*.context.json` / `*.spec.json` are skipped; at most 2,000
//!   directory entries are read. `Discovered { session, record, log }`.
//! - `select(runs, identity, all) -> Vec<Discovered>`: with `all`, every
//!   run; otherwise runs whose `parent` is `identity`, then repeatedly runs
//!   whose `parent` is `run:<session>` of a selected run. A cycle is visited
//!   once; at most 2,000 runs and 32 levels. No identity and not `all`
//!   gives nothing. Runs without a record appear only under `all`.

use super::discover::{Discovered, scan, select};
use crate::commands::recipe::run_record::{RunRecord, build, record_path, write_new};
use chrono::{TimeZone, Utc};
use serde_json::Map;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const ME: &str = "session:me";

fn record(session: &str, parent: Option<&str>) -> RunRecord {
    build(
        session,
        parent.map(str::to_string),
        Utc.with_ymd_and_hms(2026, 10, 28, 9, 0, 0).unwrap(),
        "default-workflow",
        &Map::new(),
        None,
        Path::new("/home/alice/src/app"),
    )
}

fn run(session: &str, parent: Option<&str>) -> Discovered {
    Discovered {
        session: session.to_string(),
        record: Some(record(session, parent)),
        log: None,
    }
}

fn log_only(session: &str) -> Discovered {
    Discovered {
        session: session.to_string(),
        record: None,
        log: Some(format!("/logs/{session}.log").into()),
    }
}

fn sessions(runs: &[Discovered]) -> BTreeSet<String> {
    runs.iter().map(|r| r.session.clone()).collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn private_log_dir() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    d
}

fn write_record(dir: &Path, session: &str, parent: Option<&str>) {
    write_new(&record_path(dir, session), &record(session, parent)).unwrap();
    fs::write(dir.join(format!("{session}.log")), "log\n").unwrap();
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

fn family() -> Vec<Discovered> {
    vec![
        run("a", Some(ME)),
        run("b", Some("run:a")),
        run("c", Some("run:b")),
        run("other", Some("session:someone-else")),
        run("other-child", Some("run:other")),
        run("orphan", None),
        log_only("old"),
    ]
}

#[test]
fn the_default_view_is_the_callers_runs_and_their_descendants() {
    assert_eq!(
        sessions(&select(family(), Some(ME), false)),
        set(&["a", "b", "c"])
    );
}

#[test]
fn all_shows_every_run_including_ones_without_a_record() {
    assert_eq!(
        sessions(&select(family(), Some(ME), true)),
        set(&["a", "b", "c", "other", "other-child", "orphan", "old"])
    );
    assert_eq!(sessions(&select(family(), None, true)).len(), 7);
}

#[test]
fn no_identity_and_no_all_selects_nothing() {
    assert!(select(family(), None, false).is_empty());
}

#[test]
fn a_run_id_identity_selects_that_runs_children_not_the_run_itself() {
    assert_eq!(
        sessions(&select(family(), Some("run:a"), false)),
        set(&["b", "c"])
    );
}

#[test]
fn identities_from_different_sources_never_match_each_other() {
    let runs = vec![run("x", Some("session:abc")), run("y", Some("claude:abc"))];
    assert_eq!(
        sessions(&select(runs, Some("claude:abc"), false)),
        set(&["y"])
    );
}

#[test]
fn a_cycle_is_visited_once() {
    let runs = vec![run("a", Some("run:b")), run("b", Some("run:a"))];
    assert_eq!(
        sessions(&select(runs, Some("run:a"), false)),
        set(&["a", "b"])
    );
}

#[test]
fn the_walk_stops_after_32_levels() {
    let mut runs = vec![run("r1", Some(ME))];
    for n in 2..=40 {
        runs.push(run(&format!("r{n}"), Some(&format!("run:r{}", n - 1))));
    }
    let picked = sessions(&select(runs, Some(ME), false));
    assert_eq!(picked.len(), 32);
    assert!(picked.contains("r32"));
    assert!(!picked.contains("r33"));
}

#[test]
fn the_walk_stops_after_2000_runs() {
    let mut runs: Vec<Discovered> = (0..2_600)
        .map(|n| run(&format!("w{n}"), Some(ME)))
        .collect();
    // And a cycle hanging off the tree.
    runs.push(run("loop-a", Some("run:loop-b")));
    runs.push(run("loop-b", Some("run:w0")));
    let picked = select(runs, Some(ME), false);
    assert!(picked.len() <= 2_000, "{}", picked.len());
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

#[test]
fn scan_finds_records_and_logs_without_records() {
    let d = private_log_dir();
    write_record(d.path(), "with-record", Some(ME));
    fs::write(d.path().join("legacy.log"), "old run\n").unwrap();

    let found = scan(d.path());
    assert_eq!(sessions(&found), set(&["with-record", "legacy"]));
    let rec = found.iter().find(|r| r.session == "with-record").unwrap();
    assert_eq!(
        rec.record.as_ref().and_then(|r| r.parent.as_deref()),
        Some(ME)
    );
    let legacy = found.iter().find(|r| r.session == "legacy").unwrap();
    assert!(legacy.record.is_none());
    assert_eq!(
        legacy.log.as_deref(),
        Some(d.path().join("legacy.log").as_path())
    );
}

#[test]
fn scan_skips_temporary_and_secret_files() {
    let d = private_log_dir();
    write_record(d.path(), "real", Some(ME));
    for name in [
        "real.run.json.tmp.4242",
        "x.run.json.tmp.1",
        "secret.context.json",
        "secret.spec.json",
        "notes.txt",
    ] {
        fs::write(d.path().join(name), "{}").unwrap();
    }
    assert_eq!(sessions(&scan(d.path())), set(&["real"]));
}

#[test]
fn a_record_that_fails_the_file_check_is_not_trusted() {
    let d = private_log_dir();
    write_record(d.path(), "loose", Some(ME));
    fs::set_permissions(
        record_path(d.path(), "loose"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let found = scan(d.path());
    let loose = found.iter().find(|r| r.session == "loose");
    assert!(
        loose.is_none_or(|r| r.record.is_none()),
        "a 0644 record must not be read"
    );
    // So it is never a descendant.
    assert!(select(found, Some(ME), false).is_empty());
}

#[test]
fn a_missing_log_directory_is_empty() {
    let d = private_log_dir();
    assert!(scan(&d.path().join("absent")).is_empty());
}

#[test]
fn scan_reads_at_most_2000_entries() {
    let d = private_log_dir();
    for n in 0..3_000 {
        fs::write(d.path().join(format!("s{n}.log")), "").unwrap();
    }
    let found = scan(d.path());
    assert!(found.len() <= 2_000, "{}", found.len());
}

#[test]
fn scan_never_changes_the_log_directory() {
    let d = private_log_dir();
    write_record(d.path(), "r", Some(ME));
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let before: BTreeSet<_> = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let _ = scan(d.path());
    let after: BTreeSet<_> = fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(before, after);
    assert_eq!(
        fs::metadata(d.path()).unwrap().permissions().mode() & 0o777,
        0o755,
        "the status command never chmods"
    );
}
