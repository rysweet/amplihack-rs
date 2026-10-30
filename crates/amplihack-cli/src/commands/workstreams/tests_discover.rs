//! Issue #1710 (D4): finding runs in the recipe log directory and keeping
//! the caller's descendants.
//!
//! Contract (`discover`):
//! - `scan(log_dir) -> (Vec<Discovered>, Option<&'static str>)`: one entry
//!   per session found as a readable `<session>.run.json` or a
//!   `<session>.log` owned by this user; names containing `.tmp.` and
//!   `*.context.json` / `*.spec.json` are skipped; at most 20,000 directory
//!   entries are listed and at most 2,000 run files, newest first, are read. `Discovered { session, record, log }`. The second
//!   value is a fixed warning: `UNREADABLE` for a directory that cannot be
//!   listed, `NOT_PRIVATE` (and no runs) for one not owned by this user or
//!   writable by group or others, `TRUNCATED` when older run files were left
//!   unread, `TOO_MANY_ENTRIES` when entries were left unlisted (then the
//!   runs shown are not necessarily the newest).
//!   A missing directory gives no runs and no warning.
//! - `select(runs, identity, all) -> Vec<Discovered>`: with `all`, every
//!   run; otherwise runs whose `parent` is `identity`, then repeatedly runs
//!   whose `parent` is `run:<session>` of a selected run. A cycle is visited
//!   once; at most 2,000 runs and 32 levels. No identity and not `all`
//!   gives nothing. Runs without a record appear only under `all`.

use super::discover::{
    Discovered, NOT_PRIVATE, TOO_MANY_ENTRIES, TRUNCATED, UNREADABLE, owned_by, scan, scan_capped,
    select,
};
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

    let (found, warning) = scan(d.path());
    assert_eq!(warning, None);
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
    assert_eq!(sessions(&scan(d.path()).0), set(&["real"]));
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
    let (found, _) = scan(d.path());
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
    assert_eq!(scan(&d.path().join("absent")), (Vec::new(), None));
}

#[test]
fn scan_reads_at_most_2000_entries() {
    let d = private_log_dir();
    for n in 0..3_000 {
        fs::write(d.path().join(format!("s{n}.log")), "").unwrap();
    }
    let (found, warning) = scan(d.path());
    assert!(found.len() <= 2_000, "{}", found.len());
    assert_eq!(warning, Some(TRUNCATED));
}

#[test]
fn the_cap_warns_only_when_entries_are_left_unread() {
    let d = private_log_dir();
    for n in 0..5 {
        fs::write(d.path().join(format!("s{n}.log")), "").unwrap();
    }
    let (found, warning) = scan_capped(d.path(), 20_000, 3);
    assert_eq!(found.len(), 3);
    assert_eq!(warning, Some(TRUNCATED));
    let (found, warning) = scan_capped(d.path(), 20_000, 5);
    assert_eq!(found.len(), 5);
    assert_eq!(warning, None);
}

#[test]
fn past_the_entry_limit_the_warning_does_not_claim_the_newest_runs() {
    let d = private_log_dir();
    for n in 0..5 {
        fs::write(d.path().join(format!("s{n}.log")), "").unwrap();
    }
    // Four of five entries listed: the cut happens before the sort by age,
    // so the warning must not say that only older runs are missing.
    let (found, warning) = scan_capped(d.path(), 4, 2_000);
    assert_eq!(found.len(), 4);
    assert_eq!(warning, Some(TOO_MANY_ENTRIES));
    assert!(
        !TOO_MANY_ENTRIES.contains("older runs"),
        "{TOO_MANY_ENTRIES}"
    );
    // Both limits reached: the entry limit is the one reported.
    let (_, warning) = scan_capped(d.path(), 4, 2);
    assert_eq!(warning, Some(TOO_MANY_ENTRIES));
    // Exactly at the entry limit nothing is left unlisted.
    let (found, warning) = scan_capped(d.path(), 5, 2_000);
    assert_eq!(found.len(), 5);
    assert_eq!(warning, None);
}

/// Sets the modification time of `path` to `secs` after the epoch.
fn set_mtime(path: &Path, secs: u64) {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

#[test]
fn past_the_cap_the_newest_runs_are_kept() {
    let d = private_log_dir();
    for n in 0..6u64 {
        let log = d.path().join(format!("s{n}.log"));
        fs::write(&log, "").unwrap();
        // s5 is the newest, s0 the oldest; names sort the other way from
        // age, so name order cannot pass this test by chance.
        set_mtime(&log, 1_000_000 + n * 60);
    }
    let (found, warning) = scan_capped(d.path(), 20_000, 3);
    assert_eq!(sessions(&found), set(&["s3", "s4", "s5"]));
    assert_eq!(warning, Some(TRUNCATED));
}

#[test]
fn a_run_counts_from_its_newest_file() {
    let d = private_log_dir();
    // `live` was launched long ago, so its record is old, but its log is
    // still being written. Both of its files are kept, as one run.
    write_record(d.path(), "live", Some(ME));
    set_mtime(&d.path().join("live.run.json"), 1_000);
    let log = d.path().join("live.log");
    fs::write(&log, "").unwrap();
    set_mtime(&log, 9_000_000);
    for n in 0..3u64 {
        let p = d.path().join(format!("old{n}.log"));
        fs::write(&p, "").unwrap();
        set_mtime(&p, 5_000 + n);
    }
    let (found, warning) = scan_capped(d.path(), 20_000, 3);
    assert_eq!(sessions(&found), set(&["live", "old2"]));
    let live = found.iter().find(|r| r.session == "live").unwrap();
    assert!(live.record.is_some() && live.log.is_some());
    assert_eq!(warning, Some(TRUNCATED));
}

#[test]
fn a_log_owned_by_another_user_is_not_a_run() {
    let d = private_log_dir();
    let log = d.path().join("s.log");
    fs::write(&log, "").unwrap();
    let meta = fs::symlink_metadata(&log).unwrap();
    // SAFETY: geteuid has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    assert!(owned_by(&meta, me));
    assert!(!owned_by(&meta, me.wrapping_add(1)));
}

#[test]
fn a_log_directory_others_may_write_to_is_ignored_with_a_warning() {
    for mode in [0o770, 0o702, 0o777] {
        let d = private_log_dir();
        write_record(d.path(), "r", Some(ME));
        fs::set_permissions(d.path(), fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(scan(d.path()), (Vec::new(), Some(NOT_PRIVATE)), "{mode:o}");
    }
}

#[test]
fn a_log_directory_that_cannot_be_listed_warns_without_its_path() {
    let d = private_log_dir();
    let file = d.path().join("not-a-dir");
    fs::write(&file, "").unwrap();
    assert_eq!(scan(&file), (Vec::new(), Some(UNREADABLE)));

    // SAFETY: geteuid has no preconditions. Root lists any directory.
    if unsafe { libc::geteuid() } != 0 {
        let locked = d.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let result = scan(&locked);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(result, (Vec::new(), Some(UNREADABLE)));
    }
    for warning in [UNREADABLE, NOT_PRIVATE, TRUNCATED, TOO_MANY_ENTRIES] {
        assert!(!warning.contains('/'), "{warning}");
    }
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
