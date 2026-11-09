//! Issue #2018: writing and removing a record (D5, D6 and the security
//! requirements on the writer). See `tests_support` for the contract.

#![cfg(unix)]

use super::tests_support::*;
use crate::step_record::{MAX_RECORD_BYTES, StepRecord, StepRecordGuard, record_path, root};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

fn err_text(result: anyhow::Result<StepRecordGuard>) -> String {
    format!("{:#}", result.expect_err("the write must be refused"))
}

/// A temporary home and its records root, which does not exist yet.
fn home_and_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let root = root(&home);
    (tmp, root)
}

// ---------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------

#[test]
fn a_record_is_written_with_private_modes_and_no_leftovers() {
    let (_tmp, root) = home_and_root();
    let rec = record(RUN, 4242);
    let guard = StepRecordGuard::try_write(&root, &rec).unwrap();
    let path = record_path(&root, RUN, 4242);
    assert_eq!(guard.path(), path.as_path());
    let back: StepRecord = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(back, rec);
    assert_eq!(mode(&path), 0o600, "record mode");
    assert_eq!(mode(&root), 0o700, "recipe-steps directory mode");
    assert_eq!(mode(&root.join(RUN)), 0o700, "run-id directory mode");
    assert_eq!(
        names(&root.join(RUN)),
        vec!["4242.json"],
        "no temporary file left"
    );
}

/// An existing root with a safe mode is used as it is and never chmodded.
#[test]
fn an_existing_root_keeps_its_mode() {
    let (_tmp, root) = home_and_root();
    fs::create_dir_all(&root).unwrap();
    set_mode(&root, 0o755);
    let _guard = StepRecordGuard::try_write(&root, &record(RUN, 4242)).unwrap();
    assert_eq!(mode(&root), 0o755);
}

#[test]
fn a_root_that_is_a_regular_file_is_refused() {
    let (_tmp, root) = home_and_root();
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    fs::write(&root, "not a directory").unwrap();
    let text = err_text(StepRecordGuard::try_write(&root, &record(RUN, 4242)));
    assert!(text.starts_with("unsafe directory "), "got {text:?}");
    assert!(text.contains(&root.display().to_string()), "got {text:?}");
    assert_eq!(fs::read_to_string(&root).unwrap(), "not a directory");
}

#[test]
fn a_symlinked_root_or_run_directory_is_refused_and_nothing_lands_at_the_target() {
    for link_run_dir in [false, true] {
        let (tmp, root) = home_and_root();
        let target = tmp.path().join("elsewhere");
        fs::create_dir_all(&target).unwrap();
        if link_run_dir {
            fs::create_dir_all(&root).unwrap();
            set_mode(&root, 0o700);
            symlink(&target, root.join(RUN)).unwrap();
        } else {
            fs::create_dir_all(root.parent().unwrap()).unwrap();
            symlink(&target, &root).unwrap();
        }
        let text = err_text(StepRecordGuard::try_write(&root, &record(RUN, 4242)));
        assert!(
            text.starts_with("unsafe directory "),
            "run dir {link_run_dir}: {text:?}"
        );
        assert!(
            names(&target).is_empty(),
            "run dir {link_run_dir}: wrote through the link"
        );
    }
}

/// SR-4: a group- or world-writable root or run directory is refused, and
/// the existing mode is left alone.
#[test]
fn a_group_or_world_writable_directory_is_refused() {
    for (at_root, bad_mode) in [(true, 0o777), (true, 0o770), (false, 0o777), (false, 0o720)] {
        let (_tmp, root) = home_and_root();
        let dir = if at_root {
            root.clone()
        } else {
            root.join(RUN)
        };
        fs::create_dir_all(&dir).unwrap();
        set_mode(&root, if at_root { bad_mode } else { 0o700 });
        set_mode(&dir, bad_mode);
        let text = err_text(StepRecordGuard::try_write(&root, &record(RUN, 4242)));
        assert!(
            text.starts_with("unsafe directory "),
            "{dir:?} {bad_mode:o}: {text:?}"
        );
        assert_eq!(mode(&dir), bad_mode, "the mode must not be changed");
        assert!(!record_path(&root, RUN, 4242).exists());
    }
}

/// SR-3: a directory owned by someone else is refused. The uid seam stands
/// in for another user, which a test cannot create without root.
#[test]
fn a_directory_not_owned_by_the_user_is_refused() {
    let (_tmp, root) = home_and_root();
    fs::create_dir_all(&root).unwrap();
    set_mode(&root, 0o700);
    let text = err_text(StepRecordGuard::try_write_as(
        &root,
        &record(RUN, 4242),
        other_uid(),
    ));
    assert!(text.starts_with("unsafe directory "), "got {text:?}");
    assert!(
        !root.join(RUN).exists(),
        "nothing is created under a foreign root"
    );
}

#[test]
fn an_invalid_run_id_is_refused_without_being_echoed() {
    let (tmp, root) = home_and_root();
    for id in ["../escape", "esc\u{1b}]0;x\u{7}", ""] {
        let text = err_text(StepRecordGuard::try_write(&root, &record(id, 4242)));
        assert!(
            text.starts_with("invalid recipe run id"),
            "{id:?}: {text:?}"
        );
        assert!(
            !text.contains("escape") && !text.contains('\u{1b}'),
            "{text:?}"
        );
    }
    assert!(!tmp.path().join("home/.amplihack/runtime/escape").exists());
    assert!(!root.exists(), "nothing is created for an invalid run id");
}

#[test]
fn a_record_over_64_kib_is_not_written() {
    let (_tmp, root) = home_and_root();
    let mut rec = record(RUN, 4242);
    rec.cwd = format!("/{}", "d".repeat(MAX_RECORD_BYTES));
    let text = err_text(StepRecordGuard::try_write(&root, &rec));
    assert!(text.starts_with("record too large"), "{text:?}");
    assert!(
        names(&root.join(RUN)).is_empty(),
        "no file, temporary or final"
    );
}

/// A failed rename (a directory in the way) leaves no temporary file.
#[test]
fn a_failed_write_leaves_no_temporary_file() {
    let (_tmp, root) = home_and_root();
    fs::create_dir_all(record_path(&root, RUN, 4242)).unwrap();
    set_mode(&root, 0o700);
    set_mode(&root.join(RUN), 0o700);
    let text = err_text(StepRecordGuard::try_write(&root, &record(RUN, 4242)));
    assert!(text.starts_with("cannot write "), "{text:?}");
    assert_eq!(
        names(&root.join(RUN)),
        vec!["4242.json"],
        "only the obstacle remains"
    );
}

// ---------------------------------------------------------------------
// Removal
// ---------------------------------------------------------------------

#[test]
fn drop_removes_the_record_and_the_empty_run_directory_only() {
    let (_tmp, root) = home_and_root();
    let guard = StepRecordGuard::try_write(&root, &record(RUN, 4242)).unwrap();
    let path = guard.path().to_path_buf();
    drop(guard);
    assert!(!path.exists(), "the record must be removed on drop");
    assert!(
        !root.join(RUN).exists(),
        "the empty run-id directory must go"
    );
    assert!(root.is_dir(), "the recipe-steps directory itself must stay");
}

#[test]
fn drop_keeps_a_run_directory_that_still_holds_something_else() {
    let (_tmp, root) = home_and_root();
    let first = StepRecordGuard::try_write(&root, &record(RUN, 4242)).unwrap();
    let second = StepRecordGuard::try_write(&root, &record(RUN, 4343)).unwrap();
    let notes = root.join(RUN).join("notes.txt");
    fs::write(&notes, "keep").unwrap();
    drop(first);
    assert!(
        second.path().is_file(),
        "another step's record must survive"
    );
    drop(second);
    assert_eq!(fs::read_to_string(&notes).unwrap(), "keep");
    assert!(root.join(RUN).is_dir(), "a non-empty run directory stays");
}

/// The Drop recheck: if the run directory was swapped for a symlink after
/// the write, nothing at the link's target is removed.
#[test]
fn drop_leaves_the_target_alone_when_the_run_directory_became_a_symlink() {
    let (tmp, root) = home_and_root();
    let guard = StepRecordGuard::try_write(&root, &record(RUN, 4242)).unwrap();
    fs::rename(root.join(RUN), tmp.path().join("moved")).unwrap();
    let target = tmp.path().join("victim");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("4242.json"), "precious").unwrap();
    symlink(&target, root.join(RUN)).unwrap();
    drop(guard);
    assert_eq!(
        fs::read_to_string(target.join("4242.json")).unwrap(),
        "precious"
    );
    assert!(
        fs::symlink_metadata(root.join(RUN))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

/// Drop never panics, even when its file has already gone.
#[test]
fn drop_after_the_record_was_removed_by_hand_is_quiet() {
    let (_tmp, root) = home_and_root();
    let guard = StepRecordGuard::try_write(&root, &record(RUN, 4242)).unwrap();
    fs::remove_file(guard.path()).unwrap();
    drop(guard);
    assert!(!root.join(RUN).exists());
}
