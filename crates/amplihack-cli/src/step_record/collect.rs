//! Issue #2018: removing step records that name nothing still running.
//!
//! A launcher killed with SIGKILL cannot remove its own record. The pinned
//! recipe runner kills an agent step's launcher that way whenever the step
//! times out, so such records are routine, not rare. `amplihack recipe run`
//! therefore owns the records its runner's steps leave behind:
//!
//! - [`collect_run`]: after each runner attempt has exited, the `stale`
//!   records under that attempt's run id are removed, then the run-id
//!   directory if it is empty. `live` and `orphan` records are kept, so an
//!   orphan stays visible to `recipe steps`.
//! - [`collect_abandoned`]: at the end of a run, a run-id directory in which
//!   every entry is a `stale` record last written longer ago than the
//!   cleanup keep window is removed. This collects what a `recipe run` that
//!   was itself killed with SIGKILL could not.
//!
//! A record is removed only when, read a second time immediately before the
//! removal, it is unchanged and still classified `stale` by the same
//! start-time check `recipe steps` uses, and its run-id directory and the
//! root are still private directories owned by this user. Nothing here
//! sends a signal, and nothing but a record file or an empty run-id
//! directory is ever removed.

use super::list::{Listed, list_run_dir, read_record};
use super::probe::{Probe, StepState, classify};
use super::write::check_private_dir;
use super::{StepRecord, effective_uid, valid_run_id};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Removes the `stale` records in `<root>/<run_id>`, then the directory if
/// it is now empty. Returns the record paths removed.
pub fn collect_run(root: &Path, run_id: &str, probe: &impl Probe) -> Vec<PathBuf> {
    collect_run_as(root, run_id, effective_uid(), probe)
}

/// [`collect_run`] with the uid every directory must belong to. The uid is
/// a parameter only so that tests can stand in for another user.
pub fn collect_run_as(root: &Path, run_id: &str, uid: u32, probe: &impl Probe) -> Vec<PathBuf> {
    if !valid_run_id(run_id) || !private_dir(root, uid) {
        return Vec::new();
    }
    let dir = root.join(run_id);
    let Some(records) = records_in(&dir, run_id, uid) else {
        return Vec::new();
    };
    let removed: Vec<PathBuf> = records
        .into_iter()
        .filter_map(|(path, record)| remove_if_stale(root, &dir, &path, &record, uid, probe))
        .collect();
    // Fails, as it should, while any other entry remains.
    let _ = fs::remove_dir(&dir);
    removed
}

/// Removes every run-id directory under `root` in which each entry is a
/// `stale` record whose file was last modified at least `min_age` before
/// `now`. A directory holding anything else, including an unreadable entry
/// or a `live` or `orphan` record, is left whole. Returns the record paths
/// removed.
pub fn collect_abandoned(
    root: &Path,
    min_age: Duration,
    now: SystemTime,
    probe: &impl Probe,
) -> Vec<PathBuf> {
    collect_abandoned_as(root, min_age, now, effective_uid(), probe)
}

/// [`collect_abandoned`] with the uid every directory must belong to.
pub fn collect_abandoned_as(
    root: &Path,
    min_age: Duration,
    now: SystemTime,
    uid: u32,
    probe: &impl Probe,
) -> Vec<PathBuf> {
    if !private_dir(root, uid) {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut run_ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| valid_run_id(name))
        .collect();
    run_ids.sort();

    let mut removed = Vec::new();
    for run_id in run_ids {
        let dir = root.join(&run_id);
        let Some(records) = records_in(&dir, &run_id, uid) else {
            continue;
        };
        let abandoned = records.iter().all(|(path, record)| {
            classify(record, probe) == StepState::Stale && older_than(path, min_age, now)
        });
        if !abandoned {
            continue;
        }
        if records.is_empty() && !older_than(&dir, min_age, now) {
            continue;
        }
        for (path, record) in &records {
            removed.extend(remove_if_stale(root, &dir, path, record, uid, probe));
        }
        let _ = fs::remove_dir(&dir);
    }
    removed
}

/// The readable records in one run-id directory, or `None` when the
/// directory is not a private directory owned by `uid`, or holds any entry
/// that could not be read.
fn records_in(dir: &Path, run_id: &str, uid: u32) -> Option<Vec<(PathBuf, StepRecord)>> {
    if !private_dir(dir, uid) {
        return None;
    }
    let mut found = Vec::new();
    list_run_dir(dir, run_id, uid, &mut found);
    let mut records = Vec::with_capacity(found.len());
    for (_, _, listed) in found {
        match listed {
            Listed::Record { path, record } => records.push((path, record)),
            Listed::Unreadable { .. } => return None,
        }
    }
    Some(records)
}

/// Removes `path` if it still holds `record`, the record is still `stale`,
/// and both directories are still private and owned by `uid`.
fn remove_if_stale(
    root: &Path,
    dir: &Path,
    path: &Path,
    record: &StepRecord,
    uid: u32,
    probe: &impl Probe,
) -> Option<PathBuf> {
    if classify(record, probe) != StepState::Stale {
        return None;
    }
    let unchanged = matches!(
        read_record(path, &record.run_id, record.launcher.pid),
        Listed::Record { record: again, .. } if again == *record
    );
    let still_stale = unchanged && classify(record, probe) == StepState::Stale;
    if !still_stale || !private_dir(root, uid) || !private_dir(dir, uid) {
        return None;
    }
    match fs::remove_file(path) {
        Ok(()) => Some(path.to_path_buf()),
        Err(error) => {
            tracing::debug!(%error, path = %path.display(), "stale step record not removed");
            None
        }
    }
}

/// `path` is a directory, not a symlink, owned by `uid` and not writable by
/// group or others: the same test the writer applies.
fn private_dir(path: &Path, uid: u32) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| check_private_dir(path, &meta, uid).is_ok())
}

/// `path` was last modified at least `min_age` before `now`. A time that
/// cannot be read, or one in the future, is not old.
fn older_than(path: &Path, min_age: Duration, now: SystemTime) -> bool {
    fs::symlink_metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age >= min_age)
}

/// `<HOME>/.amplihack/runtime/recipe-steps`, when `HOME` is an absolute path.
fn home_root() -> Option<PathBuf> {
    crate::commands::recipe::home_dir()
        .ok()
        .filter(|home| home.is_absolute())
        .map(|home| super::root(&home))
}

/// [`collect_run`] under this user's home with the system probe. Called by
/// `recipe run` once a runner attempt has exited.
pub fn collect_after_attempt(run_id: &str) {
    if let Some(root) = home_root() {
        let removed = collect_run(&root, run_id, &super::SystemProbe);
        if !removed.is_empty() {
            tracing::debug!(count = removed.len(), "removed stale step records");
        }
    }
}

/// [`collect_abandoned`] under this user's home with the system probe and
/// the current time. Called by `recipe run` at the end of a run.
pub fn collect_abandoned_in_home(min_age: Duration) {
    if let Some(root) = home_root() {
        let removed = collect_abandoned(&root, min_age, SystemTime::now(), &super::SystemProbe);
        if !removed.is_empty() {
            tracing::debug!(count = removed.len(), "removed abandoned step records");
        }
    }
}
