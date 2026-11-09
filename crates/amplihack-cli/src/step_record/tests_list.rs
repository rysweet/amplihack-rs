//! Issue #2018: what `list` reads, what it ignores, and how it reports what
//! it cannot read (D10 and the reference page's "Which entries are listed").
//! See `tests_support` for the contract.

#![cfg(unix)]

use super::tests_support::*;
use crate::step_record::{MAX_RECORD_BYTES, StepRecordGuard, list, root};
use serde_json::json;
use std::fs;
use std::os::unix::fs::symlink;
use std::sync::mpsc;

/// The single reason `list` gives for `<RUN>/100.json` holding `value`.
fn reason_for(value: &serde_json::Value) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let path = plant_json(tmp.path(), RUN, "100.json", value);
    let (got_path, reason) = only_unreadable(&list(tmp.path()));
    assert_eq!(got_path, path);
    reason
}

// ---------------------------------------------------------------------
// Records that are read
// ---------------------------------------------------------------------

#[test]
fn a_missing_root_lists_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(list(&tmp.path().join("absent")).is_empty());
}

#[test]
fn a_valid_record_is_listed_with_its_path() {
    let tmp = tempfile::tempdir().unwrap();
    let path = plant_json(tmp.path(), RUN, "100.json", &record_json(RUN, 100));
    let listed = list(tmp.path());
    assert_eq!(records(&listed), vec![(path.as_path(), &record(RUN, 100))]);
    assert!(unreadable(&listed).is_empty(), "{listed:?}");
}

#[test]
fn a_written_record_is_listed_back() {
    let tmp = tempfile::tempdir().unwrap();
    let root = root(tmp.path());
    let rec = record(RUN, 4242);
    let guard = StepRecordGuard::try_write(&root, &rec).unwrap();
    let listed = list(&root);
    assert_eq!(records(&listed), vec![(guard.path(), &rec)]);
}

#[test]
fn unknown_fields_null_start_ticks_and_a_null_parent_are_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let mut value = record_json(RUN, 100);
    value["added_later"] = json!({"anything": [1, 2, 3]});
    value["launcher"]["start_ticks"] = serde_json::Value::Null;
    value["parent"] = serde_json::Value::Null;
    plant_json(tmp.path(), RUN, "100.json", &value);
    let listed = list(tmp.path());
    let found = records(&listed);
    assert_eq!(found.len(), 1, "{listed:?}");
    assert_eq!(found[0].1.launcher.start_ticks, None);
    assert_eq!(found[0].1.parent, None);
}

/// SR-4 is the writer's rule only: group- or world-writable directories are
/// listed normally.
#[test]
fn a_group_writable_root_and_run_directory_are_listed_normally() {
    let tmp = tempfile::tempdir().unwrap();
    plant_json(tmp.path(), RUN, "100.json", &record_json(RUN, 100));
    set_mode(tmp.path(), 0o777);
    set_mode(&tmp.path().join(RUN), 0o777);
    assert_eq!(records(&list(tmp.path())).len(), 1);
    set_mode(tmp.path(), 0o700);
}

// ---------------------------------------------------------------------
// Records that are unreadable, with their fixed reasons
// ---------------------------------------------------------------------

#[test]
fn another_format_is_unsupported_and_checked_before_any_other_field() {
    let mut value = record_json(RUN, 100);
    value["format"] = json!(2);
    assert_eq!(reason_for(&value), "unsupported format 2");
    // A format-2 record may look nothing like format 1.
    assert_eq!(reason_for(&json!({"format": 2})), "unsupported format 2");
}

#[test]
fn a_record_without_a_format_is_missing_format() {
    let mut value = record_json(RUN, 100);
    value.as_object_mut().unwrap().remove("format");
    assert_eq!(reason_for(&value), "missing format");
}

#[test]
fn invalid_json_and_wrong_types_are_invalid_json() {
    let tmp = tempfile::tempdir().unwrap();
    let path = plant(tmp.path(), RUN, "100.json", b"{ not json");
    assert_eq!(
        only_unreadable(&list(tmp.path())),
        (path, "invalid JSON".to_string())
    );

    let mut value = record_json(RUN, 100);
    value["tool"] = json!(7);
    assert_eq!(reason_for(&value), "invalid JSON");
}

/// i32 pids: a larger value does not fit and is invalid JSON, not "gone".
#[test]
fn a_pid_above_i32_max_is_invalid_json() {
    let mut value = record_json(RUN, 100);
    value["agent"]["pid"] = json!(2_147_483_648_i64);
    assert_eq!(reason_for(&value), "invalid JSON");
}

#[test]
fn a_pid_of_zero_or_below_is_an_invalid_pid() {
    for who in ["launcher", "agent", "parent"] {
        for pid in [0, -1] {
            let mut value = record_json(RUN, 100);
            value[who]["pid"] = json!(pid);
            assert_eq!(reason_for(&value), "invalid pid", "{who}.pid = {pid}");
        }
    }
}

#[test]
fn a_record_that_does_not_match_its_path_is_unreadable() {
    let mut other_run = record_json(RUN, 100);
    other_run["run_id"] = json!("another-run");
    assert_eq!(reason_for(&other_run), "record does not match its path");
    // launcher.pid 101 in 100.json.
    assert_eq!(
        reason_for(&record_json(RUN, 101)),
        "record does not match its path"
    );
}

#[test]
fn a_file_over_64_kib_is_too_large() {
    let tmp = tempfile::tempdir().unwrap();
    let path = plant(
        tmp.path(),
        RUN,
        "100.json",
        &vec![b' '; MAX_RECORD_BYTES + 1],
    );
    assert_eq!(
        only_unreadable(&list(tmp.path())),
        (path, "record too large".to_string())
    );
}

#[test]
fn a_symlink_named_like_a_record_is_not_a_regular_file_and_never_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let elsewhere = tmp.path().join("elsewhere.json");
    fs::write(
        &elsewhere,
        serde_json::to_vec(&record_json(RUN, 7)).unwrap(),
    )
    .unwrap();
    let root = tmp.path().join("root");
    fs::create_dir_all(root.join(RUN)).unwrap();
    let link = root.join(RUN).join("7.json");
    symlink(&elsewhere, &link).unwrap();
    assert_eq!(
        only_unreadable(&list(&root)),
        (link, "not a regular file".to_string())
    );
}

#[test]
fn a_fifo_named_like_a_record_is_not_a_regular_file_and_does_not_block() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    fs::create_dir_all(root.join(RUN)).unwrap();
    let fifo = root.join(RUN).join("7.json");
    let c_path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path inside this test's temp directory.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

    let (tx, rx) = mpsc::channel();
    let lister = root.clone();
    std::thread::spawn(move || {
        let _ = tx.send(list(&lister));
    });
    // A blocking open would never return; the harness then ends the test.
    let listed = rx.recv().expect("list() panicked");
    assert_eq!(
        only_unreadable(&listed),
        (fifo, "not a regular file".to_string())
    );
}
