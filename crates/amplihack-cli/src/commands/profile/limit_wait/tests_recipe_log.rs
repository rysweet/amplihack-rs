//! Issue #2731: appending the wait's `start` and `end` lines to the run's
//! recipe log, so a waiting step is visible to whoever is watching the run
//! rather than looking hung behind the runner's heartbeat.
//!
//! The write is best effort and silent. The stderr lines already carry the
//! information, so a log that cannot be appended to never changes what the
//! step does. A path that is a symlink, not a regular file, owned by another
//! user, readable by group or others, or has more than one link is refused.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_recipe_log;`.

use super::recipe_log::{RECIPE_LOG_ENV, append_to, path_from};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const START: &str = "amplihack.recipe.usage_limit_wait start reason=usage_limit account=work \
                     reset=none until=none bound=none";
const END: &str = "amplihack.recipe.usage_limit_wait end reason=login changed";

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_default()
}

// --- the happy path ------------------------------------------------------

#[test]
fn each_call_appends_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("run.log");
    append_to(&log, START).unwrap();
    append_to(&log, END).unwrap();
    assert_eq!(read(&log), format!("{START}\n{END}\n"));
}

#[test]
fn a_new_log_is_created_private() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("run.log");
    append_to(&log, START).unwrap();
    let meta = fs::metadata(&log).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(meta.nlink(), 1);
}

#[test]
fn an_existing_log_is_appended_to_and_never_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("run.log");
    fs::write(&log, "runner output\n").unwrap();
    fs::set_permissions(&log, fs::Permissions::from_mode(0o600)).unwrap();
    append_to(&log, START).unwrap();
    assert_eq!(read(&log), format!("runner output\n{START}\n"));
}

// --- what is refused -----------------------------------------------------

#[test]
fn a_symlink_is_never_followed() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::write(&target, "").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.path().join("run.log");
    symlink(&target, &link).unwrap();
    assert!(append_to(&link, START).is_err());
    assert_eq!(read(&target), "", "nothing reached the target");
}

#[test]
fn a_hard_linked_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    fs::write(&real, "").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    let log = dir.path().join("run.log");
    fs::hard_link(&real, &log).unwrap();
    assert!(
        append_to(&log, START).is_err(),
        "a second link could redirect the write"
    );
    assert_eq!(read(&real), "");
}

#[test]
fn a_log_others_can_read_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("run.log");
    fs::write(&log, "").unwrap();
    for mode in [0o640, 0o604, 0o666] {
        fs::set_permissions(&log, fs::Permissions::from_mode(mode)).unwrap();
        assert!(append_to(&log, START).is_err(), "mode {mode:o}");
        assert_eq!(read(&log), "", "mode {mode:o}");
    }
}

#[test]
fn something_that_is_not_a_regular_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("run.log");
    fs::create_dir(&sub).unwrap();
    assert!(append_to(&sub, START).is_err());
}

#[test]
fn no_parent_directory_is_ever_created() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("missing").join("run.log");
    assert!(append_to(&log, START).is_err());
    assert!(
        !dir.path().join("missing").exists(),
        "the brick creates no directory"
    );
}

// --- which paths are used at all -----------------------------------------

#[test]
fn only_an_absolute_path_is_used() {
    assert_eq!(path_from(None), None, "unset");
    assert_eq!(path_from(Some("".into())), None, "empty");
    assert_eq!(
        path_from(Some("recipe-logs/run.log".into())),
        None,
        "a relative path would resolve against repository content"
    );
    assert_eq!(
        path_from(Some("/tmp/run.log".into())),
        Some(PathBuf::from("/tmp/run.log"))
    );
}

#[test]
fn the_marker_is_the_documented_name() {
    assert_eq!(RECIPE_LOG_ENV, "AMPLIHACK_RECIPE_LOG");
}

#[test]
fn the_runners_jsonl_file_is_never_the_target() {
    // Appending plain text to the runner's own JSONL schema would corrupt
    // it; only `AMPLIHACK_RECIPE_LOG` is ever read.
    assert_ne!(RECIPE_LOG_ENV, "AMPLIHACK_RECIPE_LOG_JSONL");
}
