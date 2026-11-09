//! Issue #2018: what `collect_run` and `collect_abandoned` remove, and what
//! they leave alone. See `collect.rs` for the rules and `tests_support` for
//! the record fixture (launcher 100 started at 1000, agent 101 at 1001,
//! parent 99 at 999, for `record(RUN, 100)`).

#![cfg(unix)]

use super::tests_support::*;
use crate::step_record::{Probe, collect_abandoned_as, collect_run_as};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::time::{Duration, SystemTime};

/// Live pids and their current start times.
#[derive(Default)]
struct FakeProbe(HashMap<i32, u64>);

impl FakeProbe {
    fn with(live: &[(i32, u64)]) -> Self {
        Self(live.iter().copied().collect())
    }
}

impl Probe for FakeProbe {
    fn exists(&self, pid: i32) -> bool {
        self.0.contains_key(&pid)
    }

    fn start_ticks(&self, pid: i32) -> Option<u64> {
        self.0.get(&pid).copied()
    }
}

const OTHER_RUN: &str = "older-run-0001";
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// A time two days from now, so every file planted by a test is older than
/// one day.
fn later() -> SystemTime {
    SystemTime::now() + 2 * DAY
}

/// A temporary root made `0700` whatever the umask, as the writer makes
/// the real one.
fn private_tmp() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    set_mode(tmp.path(), 0o700);
    tmp
}

/// A record for `run` planted as the writer would leave it: the run-id
/// directory `0700` whatever the umask.
fn plant_record(root: &std::path::Path, run: &str, launcher: i32) -> std::path::PathBuf {
    let path = plant_json(
        root,
        run,
        &format!("{launcher}.json"),
        &record_json(run, launcher),
    );
    set_mode(&root.join(run), 0o700);
    path
}

// ---------------------------------------------------------------------
// collect_run
// ---------------------------------------------------------------------

#[test]
fn a_stale_record_and_its_empty_directory_are_removed() {
    let tmp = private_tmp();
    let path = plant_record(tmp.path(), RUN, 100);

    let removed = collect_run_as(tmp.path(), RUN, euid(), &FakeProbe::default());

    assert_eq!(removed, vec![path.clone()]);
    assert!(!path.exists());
    assert!(
        !tmp.path().join(RUN).exists(),
        "the empty run directory goes too"
    );
}

#[test]
fn live_and_orphan_records_are_kept_beside_a_removed_stale_one() {
    let tmp = private_tmp();
    let live = plant_record(tmp.path(), RUN, 100);
    let orphan = plant_record(tmp.path(), RUN, 200);
    let stale = plant_record(tmp.path(), RUN, 300);
    // 100 and its parent 99 alive: live. 200 alive, parent 199 gone: orphan.
    let probe = FakeProbe::with(&[(100, 1000), (99, 999), (200, 1000)]);

    let removed = collect_run_as(tmp.path(), RUN, euid(), &probe);

    assert_eq!(removed, vec![stale.clone()]);
    assert!(live.exists() && orphan.exists() && !stale.exists());
}

#[test]
fn an_agent_still_running_keeps_the_record() {
    let tmp = private_tmp();
    let path = plant_record(tmp.path(), RUN, 100);
    let probe = FakeProbe::with(&[(101, 1001)]);

    assert!(collect_run_as(tmp.path(), RUN, euid(), &probe).is_empty());
    assert!(path.exists());
}

#[test]
fn a_reused_pid_does_not_keep_a_stale_record() {
    let tmp = private_tmp();
    let path = plant_record(tmp.path(), RUN, 100);
    // Every recorded pid exists, each with a different start time.
    let probe = FakeProbe::with(&[(100, 5), (101, 5), (99, 5)]);

    assert_eq!(collect_run_as(tmp.path(), RUN, euid(), &probe), vec![path]);
}

#[test]
fn only_the_named_run_is_touched() {
    let tmp = private_tmp();
    plant_record(tmp.path(), RUN, 100);
    let other = plant_record(tmp.path(), OTHER_RUN, 100);

    collect_run_as(tmp.path(), RUN, euid(), &FakeProbe::default());

    assert!(other.exists());
}

#[test]
fn an_unreadable_entry_leaves_the_directory_untouched() {
    let tmp = private_tmp();
    let stale = plant_record(tmp.path(), RUN, 100);
    let broken = plant(tmp.path(), RUN, "200.json", b"{not json");

    assert!(collect_run_as(tmp.path(), RUN, euid(), &FakeProbe::default()).is_empty());
    assert!(stale.exists() && broken.exists());
}

#[test]
fn files_that_are_not_records_are_never_removed() {
    let tmp = private_tmp();
    plant_record(tmp.path(), RUN, 100);
    let other = plant(tmp.path(), RUN, "notes.txt", b"keep me");

    collect_run_as(tmp.path(), RUN, euid(), &FakeProbe::default());

    assert!(other.exists());
    assert_eq!(names(&tmp.path().join(RUN)), vec!["notes.txt"]);
}

#[test]
fn a_symlinked_run_directory_is_not_followed() {
    let tmp = private_tmp();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    set_mode(&root, 0o700);
    let elsewhere = tmp.path().join("elsewhere");
    let target = plant_record(&elsewhere, RUN, 100);
    symlink(elsewhere.join(RUN), root.join(RUN)).unwrap();

    assert!(collect_run_as(&root, RUN, euid(), &FakeProbe::default()).is_empty());
    assert!(target.exists());
}

#[test]
fn a_group_writable_root_or_run_directory_is_refused() {
    let tmp = private_tmp();
    let path = plant_record(tmp.path(), RUN, 100);
    set_mode(&tmp.path().join(RUN), 0o770);
    assert!(collect_run_as(tmp.path(), RUN, euid(), &FakeProbe::default()).is_empty());
    assert!(path.exists());

    set_mode(&tmp.path().join(RUN), 0o700);
    set_mode(tmp.path(), 0o777);
    assert!(collect_run_as(tmp.path(), RUN, euid(), &FakeProbe::default()).is_empty());
    assert!(path.exists());
    set_mode(tmp.path(), 0o700);
}

#[test]
fn another_users_directory_is_refused() {
    let tmp = private_tmp();
    let path = plant_record(tmp.path(), RUN, 100);

    assert!(collect_run_as(tmp.path(), RUN, other_uid(), &FakeProbe::default()).is_empty());
    assert!(path.exists());
}

#[test]
fn an_invalid_run_id_is_refused() {
    let tmp = private_tmp();
    let root = tmp.path().join("root");
    fs::create_dir(&root).unwrap();
    set_mode(&root, 0o700);
    let outside = plant_record(tmp.path(), "outside", 100);

    assert!(collect_run_as(&root, "../outside", euid(), &FakeProbe::default()).is_empty());
    assert!(outside.exists());
}

#[test]
fn a_missing_root_or_run_directory_is_nothing_to_do() {
    let tmp = private_tmp();
    let probe = FakeProbe::default();
    assert!(collect_run_as(&tmp.path().join("absent"), RUN, euid(), &probe).is_empty());
    assert!(collect_run_as(tmp.path(), RUN, euid(), &probe).is_empty());
}

// ---------------------------------------------------------------------
// collect_abandoned
// ---------------------------------------------------------------------

#[test]
fn an_old_run_of_stale_records_is_removed_whole() {
    let tmp = private_tmp();
    let a = plant_record(tmp.path(), OTHER_RUN, 100);
    let b = plant_record(tmp.path(), OTHER_RUN, 200);

    let mut removed = collect_abandoned_as(tmp.path(), DAY, later(), euid(), &FakeProbe::default());
    removed.sort();

    assert_eq!(removed, vec![a, b]);
    assert!(!tmp.path().join(OTHER_RUN).exists());
}

#[test]
fn a_recent_run_is_kept() {
    let tmp = private_tmp();
    let path = plant_record(tmp.path(), OTHER_RUN, 100);

    let removed = collect_abandoned_as(
        tmp.path(),
        DAY,
        SystemTime::now(),
        euid(),
        &FakeProbe::default(),
    );

    assert!(removed.is_empty());
    assert!(path.exists());
}

#[test]
fn a_run_with_anything_still_running_is_kept_whole() {
    let tmp = private_tmp();
    let stale = plant_record(tmp.path(), OTHER_RUN, 100);
    let orphan = plant_record(tmp.path(), OTHER_RUN, 200);
    let probe = FakeProbe::with(&[(201, 1001)]);

    assert!(collect_abandoned_as(tmp.path(), DAY, later(), euid(), &probe).is_empty());
    assert!(stale.exists() && orphan.exists());
}

#[test]
fn a_run_with_an_unreadable_entry_is_kept_whole() {
    let tmp = private_tmp();
    let stale = plant_record(tmp.path(), OTHER_RUN, 100);
    let broken = plant(tmp.path(), OTHER_RUN, "200.json", b"[]");

    assert!(
        collect_abandoned_as(tmp.path(), DAY, later(), euid(), &FakeProbe::default()).is_empty()
    );
    assert!(stale.exists() && broken.exists());
}

#[test]
fn each_run_is_judged_on_its_own() {
    let tmp = private_tmp();
    let gone = plant_record(tmp.path(), OTHER_RUN, 100);
    let kept = plant_record(tmp.path(), RUN, 300);
    let probe = FakeProbe::with(&[(300, 1000), (299, 999)]);

    let removed = collect_abandoned_as(tmp.path(), DAY, later(), euid(), &probe);

    assert_eq!(removed, vec![gone]);
    assert!(kept.exists());
}

#[test]
fn an_empty_old_run_directory_is_removed() {
    let tmp = private_tmp();
    fs::create_dir(tmp.path().join(OTHER_RUN)).unwrap();
    set_mode(&tmp.path().join(OTHER_RUN), 0o700);

    collect_abandoned_as(tmp.path(), DAY, later(), euid(), &FakeProbe::default());

    assert!(!tmp.path().join(OTHER_RUN).exists());
}

#[test]
fn directories_with_names_that_are_not_run_ids_are_ignored() {
    let tmp = private_tmp();
    let path = plant_record(tmp.path(), "not a run id", 100);

    collect_abandoned_as(tmp.path(), DAY, later(), euid(), &FakeProbe::default());

    assert!(path.exists());
}
