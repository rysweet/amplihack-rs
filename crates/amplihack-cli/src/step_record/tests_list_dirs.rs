//! Issue #2018: how `list` treats run directories and the root, which
//! names it ignores, and the order of its entries. See `tests_support` for
//! the contract.

#![cfg(unix)]

use super::tests_support::*;
use crate::step_record::{Listed, list, list_as};
use std::fs;
use std::os::unix::fs::symlink;

// ---------------------------------------------------------------------
// Run directories and the root
// ---------------------------------------------------------------------

#[test]
fn a_symlinked_run_directory_is_reported_and_never_descended() {
    let tmp = tempfile::tempdir().unwrap();
    let elsewhere = tmp.path().join("elsewhere");
    plant_json(&elsewhere, "", "100.json", &record_json(RUN, 100));
    let root = tmp.path().join("root");
    fs::create_dir_all(&root).unwrap();
    symlink(&elsewhere, root.join(RUN)).unwrap();
    let want = (root.join(RUN), "run directory is a symlink".to_string());
    assert_eq!(only_unreadable(&list(&root)), want);
}

#[test]
fn a_run_directory_owned_by_another_user_is_reported_and_never_descended() {
    let tmp = tempfile::tempdir().unwrap();
    plant_json(tmp.path(), RUN, "100.json", &record_json(RUN, 100));
    let want = (
        tmp.path().join(RUN),
        "run directory is not owned by this user".to_string(),
    );
    assert_eq!(only_unreadable(&list_as(tmp.path(), other_uid())), want);
}

#[test]
fn a_run_directory_that_cannot_be_listed_is_one_cannot_read_entry() {
    if running_as_root() {
        return; // root reads a mode-000 directory anyway
    }
    let tmp = tempfile::tempdir().unwrap();
    plant_json(tmp.path(), RUN, "100.json", &record_json(RUN, 100));
    let run_dir = tmp.path().join(RUN);
    set_mode(&run_dir, 0o000);
    let listed = list(tmp.path());
    set_mode(&run_dir, 0o700);
    let (path, reason) = only_unreadable(&listed);
    assert_eq!(path, run_dir);
    assert!(reason.starts_with("cannot read: "), "{reason:?}");
}

#[test]
fn a_root_that_cannot_be_listed_is_the_only_entry() {
    if running_as_root() {
        return; // root reads a mode-000 directory anyway
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("root");
    plant_json(&root, RUN, "100.json", &record_json(RUN, 100));
    set_mode(&root, 0o000);
    let listed = list(&root);
    set_mode(&root, 0o700);
    let (path, reason) = only_unreadable(&listed);
    assert_eq!(path, root);
    assert!(reason.starts_with("cannot read: "), "{reason:?}");
}

#[test]
fn names_that_are_not_records_are_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let body = serde_json::to_vec(&record_json(RUN, 7)).unwrap();
    for name in [
        "7.json.0f3a.tmp",
        "README",
        "7.txt",
        "07.json",
        "0.json",
        "-7.json",
    ] {
        plant(tmp.path(), RUN, name, &body);
    }
    plant(tmp.path(), RUN, "99999999999.json", &body);
    plant(tmp.path(), RUN, "2147483648.json", &body);
    fs::create_dir_all(tmp.path().join(RUN).join("8.json")).unwrap();
    plant_json(
        tmp.path(),
        "bad.name",
        "7.json",
        &record_json("bad.name", 7),
    );
    fs::write(tmp.path().join("run-as-file"), "x").unwrap();
    assert!(list(tmp.path()).is_empty(), "{:?}", list(tmp.path()));
}

#[test]
fn entries_sort_by_run_id_then_numeric_pid_with_the_run_directory_first() {
    let tmp = tempfile::tempdir().unwrap();
    for (run, pid) in [("run-b", 5), ("run-a", 100), ("run-a", 9), ("run-a", 10)] {
        plant_json(
            tmp.path(),
            run,
            &format!("{pid}.json"),
            &record_json(run, pid),
        );
    }
    plant(tmp.path(), "run-a", "50.json", b"{ not json");
    symlink(tmp.path().join("run-a"), tmp.path().join("run-0")).unwrap();
    let listed = list(tmp.path());
    let order: Vec<String> = listed
        .iter()
        .map(|entry| match entry {
            Listed::Record { record, .. } => {
                format!("{}/{}", record.run_id, record.launcher.pid)
            }
            Listed::Unreadable { path, .. } => {
                path.strip_prefix(tmp.path()).unwrap().display().to_string()
            }
        })
        .collect();
    assert_eq!(
        order,
        vec![
            "run-0",
            "run-a/9",
            "run-a/10",
            "run-a/50.json",
            "run-a/100",
            "run-b/5"
        ]
    );
}
